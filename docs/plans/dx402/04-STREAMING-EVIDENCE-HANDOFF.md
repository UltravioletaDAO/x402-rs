---
date: 2026-08-30
tags:
  - type/handoff
  - domain/dx402
  - priority/p1
status: phase-0-done
---

# DX402 sin techo: de "durable storage de cositas chicas" a storage de verdad

> **Para:** la sesión que ejecute esto desde WSL.
> **De:** la sesión de Windows del 2026-08-30.
> **Estado:** **FASE 0 IMPLEMENTADA** (2026-08-30, sesión de WSL). La fase 1
> —streaming de verdad— sigue abierta tal como está descrita abajo.
>
> Lo que quedó en el código:
> - `DX402_MAX_BODY_BYTES`, default **32 MiB** (`DEFAULT_MAX_BODY_BYTES`). Se
>   evaluaron 64 MiB y "un par de MB"; ganó 32 por dos hechos: el único caso
>   medido que DX402 rechazó son **18 MB** (por debajo de eso el default falla la
>   razón por la que se hizo configurable), y `DurableConfig::default()` viaja en
>   procesos ajenos vía crates.io, así que el default correcto es el número más
>   chico que cubre el caso conocido con aire, no el más grande que nuestra
>   propia infra aguanta. Subirlo es una variable; bajarlo después de que alguien
>   integró es una regresión.
> - `DX402_MAX_INFLIGHT_BYTES`, default **160 MiB**: presupuesto de memoria en
>   bytes, con permisos por reserva (`EvidenceBudget`/`EvidencePermit`). **Niega,
>   no encola** — bufferear pasa antes de entregar la respuesta, así que esperar
>   un permiso demoraría una entrega ya pagada.
> - `SkipReason::Busy` (`"busy"` en el cable), separado de `anchor_failed`:
>   presupuesto lleno no es una falla del store. Los SDK py/ts **no necesitan
>   cambio**: parsean `skipped` como string abierto, no como enum cerrado.
> - `EvidenceStats` en el hook: el skip dejó de ser silencioso.
> - Piso de 16 KiB, valor impareseable → default + log, y el límite de cuerpo se
>   recorta a lo que el presupuesto banca (si no, todo cuerpo grande sería `busy`
>   para siempre y se leería como falta de capacidad y no como mala config).
> - El factor de amplificación de memoria (**x4**) pasó de estimado a
>   **medido**: `crates/x402-axum/tests/memory_amplification.rs` corre una
>   captura entera bajo un asignador que cuenta y compara el pico real contra el
>   factor que el presupuesto cobra. Falla en las **dos** direcciones — si el
>   pico supera el factor, el presupuesto admite ráfagas que no puede pagar, que
>   es exactamente el OOM que venía a evitar; si el factor sobra por más de dos
>   cuerpos, se reserva memoria que nadie usa y captures legítimas se van a
>   `busy` de gusto. El presupuesto sigue en bytes de memoria real para que la
>   estimación no sea la que manda; ahora además hay quien la vigile.
>
> Los defaults son una sola decisión, no dos: 32 MiB x5 = 160 MiB exactos, así
> que **una** captura del peor caso entra y la segunda hace skip ordenado.
>
> Verificado de paso, y desarma una alarma: **`GET /dx402/blob` no puede servir
> un objeto grande.** `key_from_pointer` rechaza punteros ajenos
> (`ForeignPointer`), así que sólo lee nuestro bucket, y a nuestro bucket sólo se
> entra por el anchor inline (~47 KB por el límite de 64 KiB). El hook sube al
> sink del vendedor. No hay SSRF ni amplificador de memoria en el facilitador, y
> el tamaño del cuerpo no toca nuestra factura de S3.
>
> Las decisiones 2 y 3 del final (el header y el versionado del sobre) **no se
> tomaron**: pertenecen a la fase 1.
>
> **Disparador:** INC-7295 de MoonPay — una respuesta de 18 MB rompió una
> pipeline de ingesta. DX402 hoy la habría rechazado con `too_large`, que es
> mejor que romper, pero no es lo que el producto promete.

## El problema en una frase

**Si a 1 MiB decimos `too_large`, DX402 no es durable storage: es durable
storage de respuestas chicas.** El objetivo es que el vendedor no tenga que
pensar en el tamaño.

## El límite hoy

> **Actualizado por la fase 0.** Lo de abajo describe el estado ANTES del cambio;
> se conserva porque el resto del documento razona sobre él. Hoy el default es
> **32 MiB** (`DEFAULT_MAX_BODY_BYTES`, `durable.rs:63`), sale de
> `DX402_MAX_BODY_BYTES`, y viene acompañado del presupuesto de memoria
> `DX402_MAX_INFLIGHT_BYTES` (160 MiB, `durable.rs:79`).

`crates/x402-axum/src/durable.rs:56` — `max_body_bytes: 1_048_576` (1 MiB
exacto). Campo de `DurableConfig`, **sin override por variable de entorno**: hay
que construir el config en código para cambiarlo.

Cuando se supera: `SkipReason::TooLarge`, va en la cabecera
`X-Durable-Evidence`, y **el cuerpo se entrega igual, completo**. Eso último no
se negocia y está protegido por un test
(`an_oversized_body_is_still_delivered_in_full`): el settlement ocurre **antes**
del hook y el nonce ya se gastó, así que un cuerpo descartado sería mercadería
pagada e irrecuperable. *"Sin evidencia" nunca puede volverse "sin mercadería".*

---

## Por qué 1 MiB. Cuatro bloqueos, no uno

Subir la constante **no alcanza**. Cada capa asume el cuerpo entero en memoria:

### 1. El buffer — `durable.rs:374`, `buffer_body()`

`body.collect()` carga todo a `Bytes`. El propio código ya avisa por qué existe
el chequeo previo de `size_hint`:

> *sin esto, `collect()` bufferea la cosa entera en memoria y solo después la
> mide, así que una descarga de varios gigabytes es un OOM de la task de 2 GB en
> vez de un skip.*

**El límite protege la memoria del proceso, no una restricción de almacenamiento.**

### 2. El cifrado — `src/dx402/envelope.rs:418`

```rust
let ciphertext = aead_seal(&cek, &body_nonce, body, payment_id);
```

**AES-256-GCM de una sola pasada.** Requiere el mensaje completo. No admite
streaming seguro: no hay forma de emitir ciphertext parcial sin exponerse a un
ataque de truncado, porque el tag de autenticación se calcula al final.

### 3. El hash — `src/dx402/mod.rs:71`

`keccak256(body)` sobre el plaintext completo, de una pasada.

Este es **el bloqueo más fácil**: keccak es una construcción de esponja y admite
`update`/`finalize` incremental. Se resuelve sin cambiar el formato.

### 4. El almacenamiento — `src/dx402/store.rs:185`

S3 con `put_object` (no multipart) y Pinata con `blob.to_vec()`. La interfaz del
trait toma `&[u8]`: **la firma misma exige el objeto entero en memoria.**

---

## El bloqueo que nadie ve venir: el hash llega tarde

Este es el que hay que decidir antes de escribir una línea.

Hoy `X-Durable-Evidence` viaja con el `contentHash` **porque el cuerpo se
bufferea primero**: se calcula todo y recién después se manda la respuesta.

En HTTP **los headers van antes del body**. Con streaming, el hash del plaintext
solo se conoce cuando el último byte ya salió — y para entonces el header ya se
envió. **No se puede tener streaming y el hash en el header.**

Tres salidas, con su costo:

| Opción | Cómo | Costo |
|---|---|---|
| **A. Trailers HTTP** | El hash va en un trailer, no en un header | Soporte de clientes pobre e inconsistente. Muchos proxies los descartan |
| **B. Header liviano + consulta** | El header lleva solo `paymentId` y `status: pending`; el hash se pide a `GET /dx402/evidence/{paymentId}` | **Recomendada.** Cambia el contrato pero de forma compatible: el endpoint ya existe. El comprador hace una llamada más |
| **C. Anclar asincrónico** | Responder ya, sellar y anclar en background | El comprador no sabe si la evidencia existió. Contradice el diseño |

**Recomiendo B**, y que el header actual se conserve tal cual para cuerpos que
entran en el buffer — así lo chico no paga el costo de lo grande y no se rompe
ningún cliente existente.

---

## El plan, en dos fases separables

### FASE 0 — El límite configurable (bajo riesgo, cubre INC-7295 hoy)

**No resuelve el problema de fondo, pero cubre el caso real que lo motivó**: 18
MB entra cómodo en memoria; 18 GB no.

1. **`max_body_bytes` leído de env**, con el patrón de configuración
   centralizada del CLAUDE.md: default en código, override por
   `DX402_MAX_BODY_BYTES`, y el valor efectivo **logueado al arrancar**. El
   modelo exacto está en `src/main.rs` con `max_body_bytes` del facilitador
   (default + `env::var` + `.max(piso)` + `tracing::info!`).
2. **Un piso y un techo de cordura**, como hace el facilitador con
   `.max(16*1024)`: un valor mal parseado no puede dejar el límite en cero ni en
   algo que garantice el OOM.
3. **Y lo que hace que subirlo sea seguro: un semáforo de concurrencia.** Con
   una task de 2 GB, un límite de 32 MiB y 60 requests grandes en paralelo son
   1,9 GB y el proceso muere. Hoy no hay nada que lo impida porque 1 MiB lo hacía
   irrelevante. **Subir el límite sin acotar la concurrencia cambia un skip
   ordenado por un OOM.**
4. **Una métrica de `too_large`.** Hoy el skip es silencioso: va en un header y
   nadie lo cuenta. Nadie sabe cuántas respuestas se quedan sin evidencia por
   tamaño — exactamente la clase de silencio que costó cuatro investigaciones
   esta semana.

**Sugerencia de default: 16 MiB**, con la advertencia de que el número correcto
depende de la memoria de la task y de la concurrencia esperada, no del tamaño
que a uno le gustaría soportar.

### FASE 1 — Streaming de verdad ("storage to storage")

Un rediseño, no un ajuste. Las cuatro capas cambian.

1. **Cifrado por chunks.** Reemplazar el AEAD de una pasada por un esquema tipo
   **STREAM** (el de Hoang–Reyhanitabar–Rogaway–Vizár, que es lo que usa `age`):
   bloques de tamaño fijo, nonce derivado del índice de bloque, y el último
   bloque marcado para que truncar el archivo se detecte.

   **Esto cambia el formato del sobre y exige versionarlo.** Lo ya anclado tiene
   que seguir abriéndose: el lector debe soportar ambos formatos, para siempre.
   Un `version` en `SealedEnvelope` y una rama en el `open`.

2. **Hash incremental.** `keccak256` con `update`/`finalize` sobre los chunks.
   Es el cambio más simple de los cuatro.

3. **Multipart en el store.** Cambiar el trait de `put(&[u8])` a algo que acepte
   un stream. S3 multipart: partes de 5 MiB a 5 GiB, hasta 10.000 partes,
   **5 TiB por objeto**. Ese pasa a ser el techo real. Pinata hay que evaluarlo
   aparte: su límite depende del plan contratado y **no lo verifiqué**.

4. **El `tee` del body.** El punto más delicado del código. Hoy se bufferea
   porque hay que hacer **dos cosas con los mismos bytes**: entregárselos al
   comprador y sellarlos. Con streaming hace falta partir el stream en dos ramas
   que consumen a distinta velocidad, con backpressure — si el cifrador va más
   lento que la red, hay que decidir si se frena la entrega o se abandona la
   evidencia. **La entrega gana siempre**, por la misma razón del test de arriba.

---

## Lo que hay que decidir antes de escribir código

1. **¿Qué techo queremos de verdad?** No es lo mismo "que entren respuestas de
   50 MB" que "que se pueda anclar un archivo de 5 GB". La fase 0 resuelve lo
   primero en un día; lo segundo es la fase 1 completa.
2. **¿Se acepta la opción B del header?** El comprador tendría que hacer una
   llamada más para cuerpos grandes. Es un cambio de contrato.
3. **¿Se versiona el sobre?** Es irreversible: una vez que hay dos formatos en
   la naturaleza, el lector los carga a ambos para siempre.
4. **¿Cuánta memoria le damos a la task?** Hoy son 2 GB con **una** task, y hay
   un rightsizing pendiente que propone bajarla. Streaming reduce la presión de
   memoria; la fase 0 la aumenta.

---

## Criterio de éxito

**Fase 0:**
- `DX402_MAX_BODY_BYTES=16777216` y un cuerpo de 10 MB se ancla y se recupera
  con el hash correcto.
- Un valor basura en esa variable arranca con el default y loguea el aviso, sin
  tumbar el proceso.
- 50 requests concurrentes de 10 MB **no** matan la task. Ojo con la redacción
  original: decía "el semáforo las serializa". La fase 0 **no serializa** —
  niega. Encolar detrás de un permiso demoraría una entrega que ya está pagada,
  así que la que no entra hace `busy` y se entrega igual, sin evidencia. El
  criterio real es que el total reservado nunca pase del presupuesto y que
  ninguna captura espere.
- La métrica de `too_large` se mueve cuando corresponde.

**Fase 1:**
- Un cuerpo de 1 GB se ancla sin que la memoria del proceso pase de unos pocos
  cientos de MB.
- Un sobre del formato viejo **sigue abriéndose** con el lector nuevo.
- Truncar el blob en S3 hace que la apertura falle, no que devuelva datos
  parciales.
- El comprador recupera el cuerpo y el `contentHash` coincide.

---

## Para ejecutarlo desde WSL

```bash
# El toolchain de Windows NO sirve para este repo: falta OpenSSL y el target
# unix-gated de sig_down.rs, y el rustc local es viejo para edition2024.
wsl.exe -d Ubuntu-24.04
cd /mnt/z/ultravioleta/dao/x402-rs

# El gate que tiene que quedar verde:
cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
cargo test -p x402-axum
cargo clippy -p x402-compliance && cargo test -p x402-compliance -- --test-threads=1
```

**Cuidados de este repo, que ya mordieron:**
- **Un push a `main` es un release**: CI despliega a producción.
- **`--test-threads=1` no es opcional** — los runners cuelgan en paralelo.
- **DX402 está OFF en producción** (`ENABLE_DX402` gobierna). Nada de esto
  cambia el comportamiento de nadie hasta que se encienda.
- **No corras el gate mientras otro agente edita** — compila un estado
  intermedio y da un falso rojo. Ya pasó.

## Archivos que toca

| Archivo | Fase |
|---|---|
| `crates/x402-axum/src/durable.rs` | 0 y 1 — el límite, el buffer, el semáforo |
| `crates/x402-axum/src/layer.rs` | 1 — el `tee` del body |
| `src/dx402/envelope.rs` | 1 — el cifrado por chunks y el versionado |
| `src/dx402/mod.rs` | 1 — el hash incremental |
| `src/dx402/store.rs` | 1 — multipart |
| `src/dx402/store_pinata.rs` | 1 — evaluar el límite del plan |
| `docs/DX402.md` | 0 — documentar el límite configurable |

## Lo que NO verifiqué

- **El límite real de Pinata** según el plan contratado.
- **Si algún cliente existente depende del `contentHash` en el header** — la
  opción B lo movería para cuerpos grandes.
- ~~**Cuánta memoria consume hoy un request grande de punta a punta.**~~
  **RESUELTO** por `tests/memory_amplification.rs` (fase 0): un asignador
  global que cuenta bytes vivos mide el pico de una captura de 4 MiB contra el
  tamaño del cuerpo, con un calentamiento previo para no cobrarle al cuerpo las
  asignaciones de una sola vez del proceso. El cuerpo se genera con bytes
  variados a propósito: uno de ceros halagaría a cualquier capa que comprima.
