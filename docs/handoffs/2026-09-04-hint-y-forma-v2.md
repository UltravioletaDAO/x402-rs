# El mensaje de error mandaba a la forma equivocada

**Fecha:** 2026-09-04 · **Worker:** x4-hint (Orca) · **Rama:** `0xultravioleta/x4-hint`
**Estado:** implementado, testeado y medido contra el binario local. **Cero push, cero deploy.**

---

## En una linea

El `400` de `/verify` le decia a TODO el mundo que mandara `paymentPayload` +
`paymentRequirements` — la forma **v1** — incluso a los cuerpos que declaraban
`x402Version: 2`, que no tienen `paymentRequirements`; ahora el hint se escribe segun
la version que el cuerpo declara, y ademas el sobre v2 se acepta sin la duplicacion
interna que nadie escribe por accidente.

Es el mismo defecto de ayer (el ejemplo publicado devolvia 400) en otra superficie: el
documento ya no miente, pero el mensaje de error seguia mandando al lugar equivocado.

---

## Lo que se midio, y contra que

Produccion `facilitator.ultravioletadao.xyz` version **2.10.0**, el **2026-09-04**,
con una oferta real de MeshRelay (Base, `exact`, 500000, payTo
`0xb2E85d7b223627Db243ae5Ad14Ea103dF38CB6aB`), firma inventada y wallet sin fondos.
Ningun pago se movio.

### ANTES — el sobre v2 con `resource` y `accepted` UNA vez (lo que escribe cualquiera)

```bash
curl -s -X POST https://facilitator.ultravioletadao.xyz/verify \
  -H 'Content-Type: application/json' -d @v2_nodup.json
```
```json
{
  "error": "Failed to deserialize VerifyRequest: data did not match any variant of untagged enum VerifyRequestEnvelope",
  "code": "invalid_request_body",
  "hint": "The body must be a JSON object with `paymentPayload` and `paymentRequirements`. Both the x402 v1 shape (\"network\": \"base\") and the v2 CAIP-2 shape (\"network\": \"eip155:8453\") are accepted. Worked examples: https://facilitator.ultravioletadao.xyz/skill.md"
}
```

Dos mentiras en una sola respuesta: `paymentRequirements` no existe en v2, y el link
apuntaba a un documento que solo publicaba v1.

### ANTES — el mismo cuerpo con el par repetido DENTRO de `paymentPayload`

```json
{"error":"contract_call_failed (ref: d4a8e0a2-5d39-4300-bb03-d102f8273c04)"}
```

Eso **no** es un exito — la firma es inventada — pero es el facilitador discutiendo el
**pago** en vez de la **forma**. Lo unico que separaba las dos respuestas era un
duplicado de datos que ya estaban en el request.

### ANTES — `/settle` con el mismo cuerpo v2, que era peor todavia

```json
{
  "error": "Failed to deserialize SettleRequest: missing field `scheme` at line 1 column 573",
  "code": "invalid_request_body",
  "details": "Check server logs for detailed field-by-field analysis",
  "hint": "The body must be a JSON object with `paymentPayload` and `paymentRequirements`, ..."
}
```

`missing field \`scheme\`` es el error del lector **v1** reportado como si fuera EL
error. Nombra un campo que el sobre v2 no tiene, y `details` mandaba al integrador a
unos logs que no puede leer.

### ANTES — la asimetria que nadie habia escrito

```bash
# el sobre v2, correcto en todo, pero con accepted.network = "base"
curl -s -X POST .../verify -d @v2_v1name.json
# -> 400 data did not match any variant of untagged enum VerifyRequestEnvelope
```

`accepted.network` deserializa como `Caip2NetworkId`, que exige
`namespace:reference`. El nombre v1 pelado se rechaza ahi. **El hint viejo afirmaba lo
contrario**, a todo el mundo, en la misma frase.

---

## DESPUES — medido contra el binario local de esta rama

Levantado con `scripts/run-local.sh` (writer lease apagado, credenciales AWS falsas).
Los `X-Forwarded-For` son para el rate limiter, que sin ALB adelante no puede
identificar al llamante.

### A) El sobre v2 sin duplicacion ya no discute la forma

```bash
curl -s -X POST http://127.0.0.1:8402/verify \
  -H 'Content-Type: application/json' -H 'X-Forwarded-For: 203.0.113.7' \
  -d @v2_nodup.json
```
```json
{ "isValid": false, "invalidReason": null }
```

Veredicto de pago, no error de forma. (Local da `isValid:false` en vez del
`contract_call_failed` de produccion porque aca no hay RPC de Base configurado; en
ambos casos el sobre ya se leyo.)

### B) El sobre duplicado sigue andando igual — el cambio es aditivo

```json
{ "isValid": false, "invalidReason": null }
```

### C) Un cuerpo v2 roto: el hint nombra `resource` y `accepted`

```json
{
  "error": "Failed to deserialize VerifyRequest: data did not match any variant of untagged enum VerifyRequestEnvelope",
  "code": "invalid_request_body",
  "hint": "This body declares `x402Version: 2`. x402 v2 is a JSON object with `paymentPayload`, `resource` and `accepted` -- v2 has no `paymentRequirements`; that is the v1 spelling. `accepted` needs `scheme`, `network`, `asset`, `amount`, `payTo` and `maxTimeoutSeconds`, and its `network` must be CAIP-2 (\"eip155:8453\"): the bare v1 name (\"base\") is refused there. `resource` needs `url`, `description` and `mimeType`. Repeating `resource` and `accepted` inside `paymentPayload` is accepted too, but not required. Worked examples: https://facilitator.ultravioletadao.xyz/skill.md"
}
```

### D) Un cuerpo v1 roto: el hint de siempre, intacto

```json
{
  "hint": "This body declares `x402Version: 1`. x402 v1 is a JSON object with `paymentPayload` and `paymentRequirements`. `network` may be written as the x402 v1 name (\"base\") or as the CAIP-2 identifier (\"eip155:8453\"), in both objects. Worked examples: https://facilitator.ultravioletadao.xyz/skill.md"
}
```

### E) Un cuerpo que no declara version: LAS DOS formas, no una al azar

```bash
curl -s -X POST http://127.0.0.1:8402/verify -d '{"nope":true}'
```
```json
{
  "hint": "The body declares no readable `x402Version`, so both shapes are listed. x402 v1 is a JSON object with `paymentPayload` and `paymentRequirements`. ... x402 v2 is a JSON object with `paymentPayload`, `resource` and `accepted` -- v2 has no `paymentRequirements`; ..."
}
```

### F) `/settle`: el mismo hint, y el `error` deja de mentir

```json
{
  "error": "Failed to deserialize SettleRequest: data did not match any variant of untagged enum VerifyRequestEnvelope",
  "code": "invalid_request_body",
  "details": "Read as the x402 v1 shape it fails with: missing field `scheme` at line 1 column 497",
  "hint": "This body declares `x402Version: 2`. ... /settle takes exactly the shape POST /verify takes; verify the payload there first. ..."
}
```

El error del lector v1 sigue disponible, pero como lo que es: una lectura alternativa
en `details`, no el diagnostico.

### G) `/settle` acepta el sobre v2 sin duplicar

```json
{ "isValid": false, "invalidReason": null }
```

### H) El `inputSchema` del MCP publica las dos formas

```bash
curl -s -X POST http://127.0.0.1:8402/mcp \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```
```
top-level required : ['x402Version', 'paymentPayload']
anyOf branches     : ['x402 v1', 'x402 v2']
v1 branch requires : ['paymentRequirements']
v2 branch requires : ['resource', 'accepted']
accepted.required  : ['scheme', 'network', 'asset', 'amount', 'payTo', 'maxTimeoutSeconds']
resource.required  : ['url', 'description', 'mimeType']
examples           : [1, 2]
accepted.network   : The chain as a CAIP-2 identifier ("eip155:8453"). CAIP-2 ONLY here...
```

Y una llamada real a la tool con el cuerpo v2 sin duplicar:

```json
{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"isValid\":false,\"invalidReason\":null}"}],"isError":false}}
```

### I) `/docs` (OpenAPI) lleva el cuerpo v2

```
v2 block present  : True
names accepted    : True
says CAIP-2 only  : True
settle notes v2   : True
```

---

## La duplicacion: se elimino, no se documento (y tambien se documento)

El encargo pedia evaluar si convenia arreglar la forma en vez de documentar la rareza.
**Convenia, y se hizo, de manera aditiva.**

`VerifyRequestV2` pedia `resource` y `accepted` dos veces porque las dos copias eran
load-bearing: la de afuera se convierte en el `PaymentRequirements` v1, la de adentro
aporta el `scheme` y el `network` que el `PaymentPayload` v1 lleva en su raiz.

### Por que una variante nueva y no un `Option`

Volver opcionales los dos campos de `PaymentPayloadV2` habria llegado hasta
`escrow.rs`, que lee `payment_payload.accepted.pay_to` y `.network` sobre un
`PaymentPayloadV2` **sin tener el sobre externo a mano** (`src/escrow.rs:515`, `:529`).
Ese es el camino del dinero. Asi que:

- `PaymentPayloadV2` no se toco.
- `VerifyRequestEnvelope` gana `V2Lean(VerifyRequestV2Lean)`, **ultima** en el enum
  `untagged`: se prueba despues de las cuatro variantes que ya parseaban, asi que
  ningun cuerpo que hoy funciona cambia de lectura. Solo contesta lo que antes era 400.
- `VerifyRequestV2Lean::to_full()` rellena el par interno desde el externo, y `to_v1()`
  pasa por ahi. No hay una segunda ruta de conversion que pueda derivar.

La rareza igual quedo escrita en `skill.md`, en el OpenAPI y en el `inputSchema`,
porque hay integradores que ya mandan la copia interna — era la unica forma que
andaba — y necesitan leer que pueden seguir haciendolo.

---

## Tests: cada afirmacion tiene el suyo, probados en los dos estados

| Test | Donde | Rojo cuando |
|---|---|---|
| `a_v2_body_is_told_about_resource_and_accepted` | `handlers.rs` | se restaura el hint fijo viejo |
| `a_v1_body_is_told_about_payment_requirements` | `handlers.rs` | idem |
| `an_unreadable_body_gets_both_shapes` | `handlers.rs` | idem |
| `the_version_is_read_off_the_payload_when_the_envelope_omits_it` | `handlers.rs` | idem |
| `the_endpoint_sentence_and_the_link_survive_every_branch` | `handlers.rs` | guardia: verde en los dos estados, correcto |
| `the_v2_example_published_in_skill_md_is_a_body_verify_accepts` | `wire_conformance.rs` | sin la variante lean |
| `the_two_published_examples_reduce_to_the_same_request` | `wire_conformance.rs` | idem |
| `the_inner_copy_changes_nothing_when_it_agrees` | `wire_conformance.rs` | idem |
| `settle_accepts_the_v2_envelope_too` | `wire_conformance.rs` | idem |
| `the_lean_variant_does_not_swallow_broken_bodies` | `wire_conformance.rs` | idem |
| `the_duplicated_v2_envelope_is_still_accepted` | `wire_conformance.rs` | **verde en los dos estados** — usa la variante V2 de siempre, que es justo lo que prueba que el cambio es aditivo |
| `the_payment_schema_describes_the_v2_envelope` | `mcp.rs` | esquema sin las ramas |
| `the_v2_example_embedded_in_the_schema_deserialises` | `mcp.rs` | idem |
| `the_schema_says_accepted_network_is_caip2_only` | `mcp.rs` | lleva la mitad discriminante: `Caip2NetworkId::from_str("base")` tiene que seguir fallando |
| `the_two_published_v2_examples_are_the_same_body` | `openapi.rs` | `/docs` y `skill.md` derivan |

Verificacion de discriminancia hecha de verdad, no asumida:

- Hint viejo restaurado detras de una env var temporal → **4 de 5 rojos**, la guardia
  verde. Restaurado el archivo desde backup.
- Variante lean deshabilitada (campo obligatorio imposible) → **5 de 6 fixtures v2
  rojas**, la del sobre duplicado verde. Restaurado.

### Gate de CI, verde

```
cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
  769 + 811 + 3 + 6 + 1 + 9 + 15 + 1  ->  0 failed
cargo clippy --locked -p x402-rs --features ... --all-targets   -> 0 errores (warnings preexistentes)
cargo clippy -p x402-compliance && cargo test -p x402-compliance -- --test-threads=1  -> 10 passed
cargo fmt -p x402-rs
```

`static/llms-full.txt` y el `digest` de `.well-known/agent-skills/index.json` se
regeneraron porque derivan de `skill.md` y sus tests lo exigen.

---

## Otros hallazgos del facilitador

### 1. `/settle` reportaba el error del lector v1 como si fuera EL error — ARREGLADO

Descripto arriba (medicion ANTES / F). Es el mismo defecto —el mensaje manda al lugar
equivocado— un campo mas alla del `hint`, asi que entraba en el encargo.

### 2. `VerifyRequestX402rNested.payment_requirements` es obligatorio y NUNCA se lee — NO TOCADO

`src/types_v2.rs:539` lo declara sin `Option`, o sea que un cuerpo x402r-nested sin ese
campo se rechaza. Pero `to_v1()` construye los requirements desde
`payment_payload.accepted` (`:639`) y no mira `self.payment_requirements` ni una vez.

Consecuencia: en la forma x402r-nested, el `paymentRequirements` de nivel superior es
**obligatorio y decorativo**. Si difiere de `paymentPayload.accepted` — otro `payTo`,
otro `amount` — el que manda es el de adentro y el de afuera se descarta en silencio.

**No lo toque a proposito.** Es el camino de x402r/escrow, y decidir cual de los dos es
la fuente de verdad es una decision con consecuencias de dinero, no un arreglo de
documentacion. Queda anotado para que alguien lo decida con intencion.

### 3. La asimetria de `accepted.network` — DOCUMENTADA

`accepted.network` es CAIP-2 y solo CAIP-2; el `paymentRequirements.network` v1 y el
`paymentPayload.network` v1 aceptan las dos escrituras. Ademas los rechazos no ocurren
en el mismo lugar: `"base"` muere en la sintaxis, `"eip155:99999999"` parsea limpio y
recien despues no resuelve a ninguna cadena. Escrito en `skill.md`, en el OpenAPI, en
el `inputSchema` y en la guardia de `wire_conformance.rs`.

---

## Lo que es de MeshRelay / Paybox y NO se toco

El handoff `HANDOFF_MESHRELAY_X402_V2_UVD_VERIFY_BUG.md` trae, en su seccion **Root
cause**, esta instruccion para MeshRelay:

> The Facilitator expects:
> ```json
> { "x402Version": 2, "paymentPayload": {}, "paymentRequirements": {} }
> ```

**Eso es incorrecto para un flujo v2**, y lo mismo vale para el helper
`buildUvdVerifyRequest()` y el bloque "Expected flow" que propone. Para
`x402Version: 2` el facilitador toma `paymentPayload` + `resource` + `accepted`; no hay
`paymentRequirements` en v2. (Un cuerpo con `paymentRequirements` y `x402Version: 2`
solo entra por la variante x402r-nested, que es el camino de escrow y exige
`paymentPayload.payload.authorization` — no es lo que MeshRelay esta armando.)

**MeshRelay: cambien esas tres piezas del handoff por la seccion "The same payment in
the x402 v2 shape" de <https://facilitator.ultravioletadao.xyz/skill.md>** una vez que
esta rama este en produccion. Hasta entonces la forma esta en `static/skill.md` de este
repo.

Lo demas del handoff (retener el `accepts` elegido, no mutar la autorizacion, no mandar
el `PAYMENT-SIGNATURE` crudo como cuerpo, la idempotencia del retry) es correcto y
sigue valiendo.

Nada de MeshRelay ni de Paybox se modifico en esta rama.

---

## Commits (6, granulares, sin push)

```
170cb436 test(wire): fixtures del sobre v2, medidas contra produccion el 2026-09-04
3195fe39 feat(mcp): el inputSchema describe las DOS formas, no solo la v1
be2345b2 docs(openapi): el cuerpo v2 en /docs, y /settle dice que lo toma igual
e28bad5e docs(skill): publicar la forma v2 de /verify, con la duplicacion explicada
fca2e37f fix(verify,settle): el hint del 400 dice la verdad segun la version del cuerpo
24e3c5a5 feat(v2): accept the x402 v2 envelope without the inner resource/accepted
```

Archivos: `src/types_v2.rs`, `src/handlers.rs`, `src/mcp.rs`, `src/openapi.rs`,
`static/skill.md`, `static/llms-full.txt`,
`static/.well-known/agent-skills/index.json`, `tests/wire_conformance.rs`.

**Cero push. Cero deploy. Cero terraform.** `git status` limpio salvo artefactos de
build preexistentes (`target/`, `node_modules/`, `contracts/`).

### Nota de entorno

El worktree venia con el `.git` apuntando a `Z:/...` (git de Windows). Reescrito a
`gitdir: /mnt/z/...` segun la regla del dueño. Ademas el checkout esta en CRLF y el git
de WSL no tenia `autocrlf`, asi que los ~300 archivos aparecian como modificados; se
activo `core.autocrlf=true` **solo para este worktree** (`extensions.worktreeConfig`),
sin tocar la config compartida ni los otros worktrees. Lo commiteado es LF, verificado.

---

## Para c0der

El `orca orchestration send` no corre en esta terminal: contesta *"Could not connect to
the running Orca app / Orca is not running. Run 'orca open' first."*, tanto para
`heartbeat` como para `worker_done`. Por eso el cierre va aca.

**Resultado: los cinco criterios cumplidos.** (1) El hint dice la verdad para v1, para
v2 y para el cuerpo que no declara version, con cinco tests y discriminancia
verificada. (2) La forma v2 publicada en `skill.md`, en el OpenAPI y en el
`inputSchema` del MCP, atada por tests a lo que `/verify` acepta de verdad. (3) La
duplicacion **eliminada** de forma aditiva —el sobre duplicado sigue andando byte por
byte— y ademas documentada. (4) `cargo test` verde con las features de CI, mas clippy y
`x402-compliance`. (5) Este handoff con los curl de antes y despues.

**Lo que necesita tu decision:** el hallazgo #2 de arriba —
`VerifyRequestX402rNested.payment_requirements` obligatorio y nunca leido, con
`paymentPayload.accepted` ganando en silencio si difieren. Es camino de escrow y toca
dinero; no lo toque.

**Lo que hay que avisar afuera:** MeshRelay tiene tres piezas incorrectas en su propio
handoff (seccion "Lo que es de MeshRelay / Paybox"). No las toque; hay que decirselo.

Falta unicamente tu push y tu merge.
