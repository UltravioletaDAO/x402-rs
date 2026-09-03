# La friccion del experimento ChatGPT + Paybox + MeshRelay, implementada

**Fecha:** 2026-09-03
**Rama:** `0xultravioleta/x4-implementa` (worktree `x4-implementa`, WSL)
**Plan ejecutado:** `docs/handoffs/2026-09-03-plan-friccion-chatgpt.md`, items V1 a V4
**Estado:** los cuatro implementados, cada uno con su test discriminante probado en
los dos estados. Suite del repo en verde. **Cero push, cero deploy.**

---

## 1. El criterio de exito, medido

> "un agente tiene que poder: leer /skill.md, copiar el ejemplo, mandarlo a /verify y
> recibir 200."

Contra el binario de esta rama, levantado local y aislado de produccion
(`ENABLE_WRITER_LEASE=false`, credenciales AWS falsas, tabla de nonces inventada):

```
### V1 -- leer /skill.md, copiar el ejemplo, POSTearlo
prueba 1  ejemplo publicado, literal   -> HTTP 200  {"isValid":false,"invalidReason":null,"payer":"0x0000000000000000000000000000000000000001"}
prueba 2  forma v1 + "base"            -> HTTP 200  {"isValid":false,...}
prueba 3  forma v1 + "eip155:8453"     -> HTTP 200  {"isValid":false,...}
prueba 3b el mismo cuerpo por /settle  -> HTTP 200  {"isValid":false,...}

### V1 -- una oferta REAL del Bazaar de produccion, sin reescribir el nombre de la red
bazaar    eip155:8453 -> /verify       -> HTTP 200  {"isValid":false,...}
```

El cuerpo de la prueba 1 no esta tipeado a mano: lo saca un script del `/skill.md`
que sirve ese mismo binario, exactamente como lo haria el agente.

Las mismas tres pruebas contra **produccion 2.10.0**, antes de esta rama:

```
prueba 1  -> HTTP 400  {"error":"Failed to deserialize VerifyRequest: data did not match
                        any variant of untagged enum VerifyRequestEnvelope",
                        "code":"invalid_request_body",
                        "hint":"... Both the x402 v1 shape (\"network\": \"base\") and the
                                v2 CAIP-2 shape (\"network\": \"eip155:8453\") are accepted.
                                Worked examples: .../skill.md"}
prueba 2  -> HTTP 200  {"isValid":false,"invalidReason":null,"payer":"0x...01"}
prueba 3  -> HTTP 400  identico a la prueba 1
```

El `hint` de ese 400 prometia que las dos escrituras se aceptaban y mandaba a leer el
documento del ejemplo roto. Ahora las dos frases son ciertas.

---

## 2. Que se hizo, commit por commit

| Commit | Item | Toca |
|---|---|---|
| `aa0954e3` | **V1** | `network.rs`, `types.rs`, `skill.md`, `openapi.rs`, `llms-full.txt`, digest, sitemap |
| `c701e181` | **V2** | `handlers.rs`, `network.rs`, `facilitator_local.rs`, `openapi.rs`, `mcp.rs`, `skill.md`, + generados |
| `8cf2d9c7` | **V3** | `mcp.rs`, `skill.md`, + generados |
| `997d5305` | **V4** | `tests/wire_conformance.rs` (nuevo) |
| `56c96513` | estilo | rustfmt sobre lo que agregaron los cuatro |

Cada commit deja el repo desplegable y lleva el contrato publicado en el MISMO commit
que el codigo que describe.

### V1 -- el sobre mixto entra, y el ejemplo publicado se puede pagar

Dos defectos del mismo contrato:

**(a) Nuestro propio ejemplo devolvia 400.** Seis cosas mal a la vez. El plan enumero
cinco; **la sexta no la vio** y es la mas silenciosa:

> `validAfter` / `validBefore` iban como **numeros JSON**. `UnixTimestamp`
> (`src/timestamp.rs:24-35`) solo deserializa **strings**: `String::deserialize` y
> despues `parse::<u64>()`. `1700000000` se rechaza; `"1700000000"` se acepta.

Las otras cinco, como las listo el plan: el `paymentPayload` sin
`x402Version`/`scheme`/`network` en su raiz, `scheme`/`network`/`asset` metidos dentro
de `payload`, `amount` en lugar de `value`, sin el envoltorio `authorization`, y el
`paymentRequirements` sin `resource`, `description`, `mimeType` ni `maxTimeoutSeconds`.

El ejemplo nuevo es **ejecutable**: firma y nonce son placeholders bien formados
(`r` todo `0x11`, `s` todo `0x22`, nonce con un uno), asi que copiado literal contesta
200 con `isValid:false`. Verificado contra produccion ANTES de esta rama: el ejemplo
corregido ya daba 200 en 2.10.0, o sea que la mitad documental del arreglo sirve sola.

**(b) La forma v1 con nombre CAIP-2 no entraba.** `network::deserialize_v1_or_caip2`
(`src/network.rs`) prueba **primero** el nombre v1 por el impl derivado de serde -- no
por `FromStr` -- para que el conjunto de nombres v1 aceptados sea identico al de antes;
esto solo puede sumar. Colgado de los dos unicos puntos de entrada,
`PaymentPayload.network` y `PaymentRequirements.network`. `/settle` comparte el sobre
(`pub type SettleRequestEnvelope = VerifyRequestEnvelope`), asi que un arreglo cubre
los dos endpoints.

Es un **alias, no una normalizacion**: el chainId que se firma vive en el dominio
EIP-712, no en este campo.

### V2 -- `/accepts` dice que descarto y por que

```
### V2 -- /accepts con cuatro requirements, uno servible
accepts : 1 enriquecidos
rejected: index=0 exact     cosmos:hub-4  network_unknown
rejected: index=1 teleport  base          scheme_unknown
rejected: index=2 escrow    solana        network_unsupported

### V2 -- todo soportado
accepts: 1 | rejected: []
```

Antes (produccion 2.10.0, medido hoy): `cosmos:hub-4` y `escrow`+`base` devolvian los
dos `{"x402Version":1,"accepts":[],"error":""}` con HTTP 200 -- el mismo cuerpo que un
exito vacio.

Vocabulario cerrado de cinco: `malformed`, `network_unknown`, `scheme_unknown`,
`network_unsupported`, `scheme_unsupported_on_network`. `reason` es para hacer switch;
`detail` es prosa y puede cambiar. En `scheme_unsupported_on_network` el `detail`
nombra que SI se sirve en esa red, que es lo unico accionable sin volver a `/supported`.

**Lo que NO cambia, a proposito:** el status sigue 200 y `error` sigue vacio. Llenar
`error` cuando se descarto todo seria mas util pero rompe a un middleware que hoy hace
`if (resp.error) throw`. El plan pidio aditivo. Queda como follow-up (seccion 6).

Refactor necesario para poder probarlo: la decision entera salio del handler a
`negotiate_accepts(accepts, kinds) -> (enriched, rejected)`. Adentro solo era
alcanzable por un `Facilitator` vivo y no tenia ningun test.

### V3 -- el `inputSchema` de MCP publica la forma real

```
paymentPayload.payload.authorization: ['from', 'nonce', 'to', 'validAfter', 'validBefore', 'value']
paymentRequirements.required        : ['scheme', 'network', 'maxAmountRequired', 'resource',
                                       'description', 'mimeType', 'payTo', 'maxTimeoutSeconds', 'asset']
el ejemplo del schema es un objeto  : True
x402_accepts menciona rejected      : True
```

Antes: `{"type":"object","additionalProperties":true}` y un link a `/skill.md`, que
publicaba el ejemplo roto. Un agente por MCP no tenia NINGUNA fuente correcta del
cuerpo.

**El ejemplo del schema no es una cuarta copia.** Se saca de `static/skill.md` con un
`Lazy`, el unico lugar donde el ejemplo esta escrito. Por construccion un cliente MCP y
un lector humano no pueden ver cuerpos distintos -- que es exactamente el defecto que
este cambio arregla.

Los nested siguen con `additionalProperties: true`: el handler autodetecta v1, v2,
x402r y x402r-nested y lleva extensiones (`refund`, `upto`, `action` de escrow) adentro.

### V4 -- `tests/wire_conformance.rs`, nueve fixtures

Nombre deliberadamente distinto de `crates/x402-compliance`, que es screening de
sanciones y no conformidad de protocolo (`CLAUDE.md` la describe mal; queda anotado).

Ninguna fixture toca red, RPC ni wallet: todas parsean un cuerpo. Corre en CI tal cual.

---

## 3. Los tests, probados en rojo

Un test que pasa en los dos estados no prueba nada. Los cuatro se probaron con el
arreglo puesto y sacado.

**V1, sacando el deserializador (`deserialize_with` fuera de `types.rs`):**

```
test a_body_may_mix_the_two_spellings ................................. FAILED
test a_caip2_network_name_in_the_v1_shape_is_accepted ................. FAILED
test a_caip2_offer_from_our_own_bazaar_becomes_payable ................ FAILED
test an_unknown_network_is_still_rejected ............................. ok
test both_spellings_resolve_to_the_same_network ....................... FAILED
test every_v1_name_keeps_its_meaning_and_gains_its_twin ............... ok
test settle_accepts_exactly_what_verify_accepts ....................... FAILED
test the_example_published_in_skill_md_is_a_body_verify_accepts ....... ok
test the_v1_network_name_is_accepted .................................. ok
test result: FAILED. 4 passed; 5 failed
```

**V1, poniendo de vuelta el ejemplo viejo en `skill.md`:** 6 de 9 en rojo, incluido el
control `the_v1_network_name_is_accepted` -- porque el cuerpo de todas las fixtures sale
del documento publicado, no de una copia al lado del assert.

**V1, poniendo de vuelta el ejemplo viejo en `openapi.rs`:**

```
test openapi::tests::the_documented_verify_example_is_a_body_verify_accepts ... FAILED
test openapi::tests::the_two_published_verify_examples_are_the_same_body ...... FAILED
```

**V2, neutralizando el reporte (`rejected` siempre vacio):**

```
test an_unservable_requirement_is_no_longer_indistinguishable_from_success ... FAILED
test an_unserved_pair_names_the_schemes_that_are_served_there ................ FAILED
test both_spellings_of_a_chain_negotiate_the_same ........................... ok
test each_way_of_failing_gets_its_own_reason ................................ FAILED
test rejections_point_back_at_the_requirement_they_came_from ................ FAILED
test result: FAILED. 1 passed; 4 failed
```

**V3, volviendo al schema opaco:**

```
test mcp::tests::the_example_embedded_in_the_schema_deserialises ....... FAILED
test mcp::tests::the_schema_publishes_the_authorization_fields ......... FAILED
test mcp::tests::the_schema_says_both_network_spellings_are_accepted ... FAILED
test result: FAILED. 0 passed; 3 failed
```

**Suite completa, estado final:**

```
cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
  764 passed (lib) / 802 passed (bin) / 3 / 6 / 1 / 9 (wire_conformance) / 9 / 1  -- 0 failed
cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1
  21 / 1 / 14 / 10 / 5 / 0 / 5 -- 0 failed
cargo clippy -p x402-rs --features ... --all-targets   -- sin errores (328 warnings preexistentes)
cargo fmt -p x402-rs -- --check  -- 7 diffs, los 7 en lineas preexistentes que no toque
```

---

## 4. Donde el plan se quedo corto o el briefing cambio

**4.1 -- El plan no vio el sexto defecto del ejemplo.** `validAfter`/`validBefore` como
numeros. `src/timestamp.rs:24-35` solo deserializa strings. Cualquiera de los seis
defectos, solo, produce el mismo 400.

**4.2 -- PR #16 llego a produccion DURANTE esta sesion.** El plan midio "cero
`networkAliases`" y tenia razon al escribirse; el briefing dijo que ya estaba y tambien
tenia razon, solo que el rollout no habia terminado. Medido aca:

| Hora (EDT) | `/supported \| grep -c networkAliases` | `escrow`+`base` en `/accepts` |
|---|---|---|
| 17:36 | 0 | `{"accepts":[]}` |
| 17:52 | 1 | enriquecido, con escrow/operator/tokenCollector |

Estado final de produccion: **150 kinds, 150 con `networkAliases`, 78 identificadores
unicos**, `/version` = `2.10.0` (PR #16 no bumpeo `VERSION`).

Consecuencia: `static/skill.md:53` afirmaba que `escrow`/`commerce`/`upto` solo se
listan bajo CAIP-2, y dejo de ser cierto. Corregido en el commit de V1.

**4.3 -- Una entrada del Bazaar NO es un `PaymentRequirements`.** El plan pidio
verificar que un recurso del Bazaar se pueda pagar de punta a punta. Se verifico y da
200. Pero midiendolo aparecio algo que el plan no dice: la entrada `accepts` del
catalogo es una **oferta compacta**, no un requirements. Verbatim de
`/discovery/resources`:

```json
{"scheme":"exact","network":"eip155:8453",
 "asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
 "amount":"1000000",
 "payTo":"0x80238a1C73367591BF17e2f4DBAc652e479b077A",
 "maxTimeoutSeconds":60}
```

Le falta `resource`, `description`, `mimeType`, y su monto se llama `amount` y no
`maxAmountRequired`. El nombre de la red era la mitad que el llamador **no podia**
arreglar por su cuenta -- y es la que se arreglo. El renombre lo puede hacer el
llamador, pero hoy no esta documentado en ningun lado. Fila de backlog abajo.

**4.4 -- Las lineas del plan estaban bien.** `src/types.rs:696` y `:1423` son los dos
unicos puntos de entrada; `:1543` es `SettleResponse.network` (respuesta) y `:1944` es
`TokenAsset`, que no deriva serde. Verificado.

**4.5 -- V4 no puede regresar V2 ni V3 desde un test de integracion.**
`negotiate_accepts` es privada de `handlers` y `mcp` es un modulo del **binario**
(`src/main.rs:79`), no de la lib. Sus tests viven al lado de su codigo, que ademas es la
ubicacion correcta: el test vive donde vive la decision. La suite de V4 es la del
**cable** -- la forma que `/verify` y `/settle` aceptan.

---

## 5. El defecto P0 que aparecio midiendo, y NO se arreglo

**`invalidReason` sale siempre `null`. Tres rechazos distintos son indistinguibles, y
nuestro propio deserializador rechaza esa respuesta.**

Medido contra el binario local, tres causas de rechazo genuinamente distintas:

```
red del payload != red de los requirements -> {"isValid":false,"invalidReason":null,"payer":"0x...01"}
asset desconocido                          -> {"isValid":false,"invalidReason":null,"payer":"0x...01"}
ventana validBefore vencida                -> {"isValid":false,"invalidReason":null,"payer":"0x...01"}
```

Y produccion 2.10.0 contesta lo mismo.

**Mecanismo.** `VerifyResponse`'s `Serialize` (`src/types.rs:1669-1694`) SIEMPRE escribe
`invalidReason`. `FacilitatorErrorReason` (`src/types.rs:1509-1530`) es
`#[serde(untagged)]` con cuatro variantes unit; una variante unit en un enum untagged
se serializa como `null` y el `#[serde(rename = "insufficient_funds")]` no aplica. Solo
`FreeForm(String)` sale como texto.

**Lo peor:** el `Deserialize` de `VerifyResponse` (`src/types.rs:1697-1732`) rechaza
explicitamente ese cuerpo:

```rust
(false, None) => Err(serde::de::Error::custom(
    "`invalidReason` must be present when `isValid` is false",
)),
```

O sea: **el facilitador emite una respuesta que su propio parser no puede leer.** Un
cliente construido sobre `x402-rs` / `x402-reqwest` no puede deserializar un rechazo.
Es la misma clase de defecto que V1 -- un contrato que se contradice -- y pega de lleno
en el camino ChatGPT/Paybox: el agente recibe `isValid:false` y no puede saber por que.

**Por que no lo arregle:** el despacho enumera V1 a V4 y el arreglo cambia el formato de
cable de TODO rechazo. Es una decision de Saul, no mia. **Es el P0 numero uno de la
proxima tanda.**

El arreglo es corto: un `Serialize` a mano para `FacilitatorErrorReason` que emita el
nombre snake_case, y sacar `untagged` o dejarlo solo para `FreeForm`. Ojo con dos cosas
al hacerlo: (1) `static/skill.md` hoy documenta `{"isValid": false, "invalidReason":
null}` como respuesta de ejemplo -- es exacto hoy y hay que cambiarlo junto con el
codigo; (2) `SettleResponse.error_reason` usa el MISMO enum y tiene el mismo problema.

---

## 6. Backlog

| Date | Item | Context | Priority | Status |
|---|---|---|---|---|
| 2026-09-03 | `invalidReason` sale `null` para las cuatro razones fijas | `types.rs:1509` untagged + `types.rs:1669` serialize; el deser de `types.rs:1697` rechaza el cuerpo que emitimos. Afecta tambien `SettleResponse.errorReason` | **P0** | Pendiente |
| 2026-09-03 | Llenar `error` en `/accepts` cuando se descarto TODO | Hoy queda `""`. Mas util, pero rompe a un cliente que hace `if (resp.error) throw`. Decision de Saul | P1 | Pendiente |
| 2026-09-03 | Documentar el mapeo oferta-del-Bazaar -> `paymentRequirements` | `amount` -> `maxAmountRequired`, y hay que agregar `resource`/`description`/`mimeType`. Hoy no esta escrito en ningun lado | P1 | Pendiente |
| 2026-09-03 | Bazaar por MCP, solo lectura (4 tools) | **Desbloqueado por V1**: el catalogo es CAIP-2 y ahora eso se puede pagar | P1 | Desbloqueado |
| 2026-09-03 | `paymentId` en `TransactionRecord`, reusando `keccak256(caip2 ‖ txHash)` de `dx402/mod.rs:82-85` | Prerequisito compartido de P0.4, P0.5 y §9 del handoff de ChatGPT | P1 | Pendiente |
| 2026-09-03 | `CLAUDE.md` describe `crates/x402-compliance` como "x402 protocol conformance suite" | Es screening de sanciones. La suite de conformidad ahora es `tests/wire_conformance.rs` | P2 | Pendiente |
| 2026-09-03 | 7 diffs de rustfmt preexistentes | `handlers.rs` 2443, 2450, 14310, 14468, 14570, 14715 y `mcp.rs` 1233 | P2 | Pendiente |

---

## 7. Como reproducir todo esto

```bash
cd /mnt/c/Users/lxhxr/orca/workspaces/x402-rs/x4-implementa

# el binario, aislado de produccion (sin esto se pelea por el writer lease REAL)
cp -n config/blacklist.json.example config/blacklist.json
CARGO_TARGET_DIR=$HOME/x4-target cargo build --locked \
  --features solana,near,stellar,algorand,sui,xrpl
HOST=127.0.0.1 PORT=8402 RUST_LOG=warn SIGNER_TYPE=private-key \
  ENABLE_WRITER_LEASE=false ENABLE_WRITER_FORWARD=false \
  NONCE_STORE_TABLE_NAME=local-dev-never-a-real-table \
  AWS_ACCESS_KEY_ID=local AWS_SECRET_ACCESS_KEY=local AWS_REGION=us-east-2 \
  EVM_PRIVATE_KEY_TESTNET="0x$(python3 -c 'import secrets;print(secrets.token_hex(32))')" \
  EVM_PRIVATE_KEY_MAINNET="0x$(python3 -c 'import secrets;print(secrets.token_hex(32))')" \
  RPC_URL_BASE_SEPOLIA=https://sepolia.base.org RPC_URL_BASE=https://mainnet.base.org \
  $HOME/x4-target/debug/x402-rs &

# el criterio de exito, sin tipear el cuerpo a mano
X='X-Forwarded-For: 192.0.2.1'   # sin ALB, tower_governor contesta 500 sin esto
curl -s -H "$X" http://127.0.0.1:8402/skill.md \
  | python3 -c "import sys;m=sys.stdin.read();print(m.split('## 3. \`POST /verify\`',1)[1].split('\`\`\`json',1)[1].split('\`\`\`',1)[0].strip())" \
  > /tmp/body.json
curl -s -w '\nHTTP %{http_code}\n' -H "$X" -H 'Content-Type: application/json' \
  -X POST http://127.0.0.1:8402/verify -d @/tmp/body.json
sed 's/"base"/"eip155:8453"/g' /tmp/body.json > /tmp/body-caip2.json
curl -s -w '\nHTTP %{http_code}\n' -H "$X" -H 'Content-Type: application/json' \
  -X POST http://127.0.0.1:8402/verify -d @/tmp/body-caip2.json
```

**Notas de entorno (WSL, worktree en `/mnt/c`):**
- El `.git` del worktree venia con una ruta Windows y el git de WSL no lo abria. Se
  reescribio a `gitdir: /mnt/z/ultravioleta/dao/x402-rs/.git/worktrees/x4-implementa`.
- **Todo el worktree esta con CRLF y el indice con LF**, asi que `git status` muestra
  407 archivos sucios. Todos los comandos de git de esta sesion corrieron con
  `git -c core.autocrlf=input`, que deja el arbol limpio y no cambia nada de lo
  commiteado. Los commits salieron con LF, como el resto del repo.
- `scripts/build_llms_full.sh` no corre tal cual: tiene CRLF y `set -euo pipefail\r`
  revienta con `invalid option name`. Se corrio sobre una copia sin CR. **No es un
  defecto del script, es el checkout.**
- `CARGO_TARGET_DIR` en `$HOME` (ext4). Con el target en `/mnt/c` sobre 9P la
  compilacion es varias veces mas lenta.

---

## Para c0der

`orca orchestration` no salio en toda la sesion ("Could not connect to the running Orca
app / Orca is not running"), asi que el cierre va aca, como pide el despacho.

**Que quedo.** Los cuatro items VALE LA PENA del plan estan implementados en
`0xultravioleta/x4-implementa`, en cinco commits, en el orden que el plan los numera y
cada uno dejando el repo desplegable: `aa0954e3` V1 (el sobre mixto entra y el ejemplo
publicado se puede pagar), `c701e181` V2 (`/accepts` devuelve `rejected[]` con motivo),
`8cf2d9c7` V3 (el `inputSchema` de MCP publica la forma real del sobre), `997d5305` V4
(`tests/wire_conformance.rs`, nueve fixtures), `56c96513` rustfmt de lo agregado. Los
contratos publicados -- `skill.md`, `openapi.rs`, la descripcion de las tools MCP,
`llms-full.txt`, el digest de agent-skills y el sitemap -- van en el mismo commit que el
codigo que describen. Suite en verde: 764 lib + 802 bin + integracion, mas
axum/reqwest/compliance; clippy sin errores.

**Que se encontro.** El criterio de exito se cumple y esta pegado arriba con la salida
real: un agente lee `/skill.md`, copia el ejemplo, lo POSTea y recibe 200 -- y tambien
lo recibe con `"eip155:8453"`, y por `/settle`, y con una oferta REAL sacada del Bazaar
de produccion sin reescribirle el nombre de la red. Los cuatro tests se probaron en
rojo sacando cada arreglo (V1: 5 de 9 fixtures caen; con el ejemplo viejo, 6 de 9; V2:
4 de 5; V3: 3 de 3). Tres cosas que el plan no traia: el ejemplo tambien fallaba por
mandar los timestamps como numeros (`timestamp.rs:24-35` solo deserializa strings); PR
#16 llego a produccion a mitad de sesion (17:36 EDT sin `networkAliases`, 17:52 con), lo
que dejo stale `skill.md:53` y se corrigio; y una entrada del Bazaar es una oferta
compacta, no un `PaymentRequirements` -- le falta `resource`/`description`/`mimeType` y
su monto se llama `amount`.

**Que falta, y hay un P0 nuevo.** **`invalidReason` sale siempre `null`**: tres causas
de rechazo distintas (red que no matchea, asset desconocido, ventana vencida) devuelven
el mismo cuerpo, y el `Deserialize` de nuestro propio `VerifyResponse`
(`types.rs:1697-1732`) rechaza explicitamente ese cuerpo -- emitimos una respuesta que
nuestro propio parser no puede leer. Es la misma clase de defecto que V1 y pega de lleno
en el camino de ChatGPT/Paybox. No lo toque porque cambia el formato de cable de TODO
rechazo y esa es tu decision; el detalle, el mecanismo y el arreglo estan en la seccion
5. Ademas: **no bumpee `VERSION`** (sigue en `2.10.0`, igual que produccion, porque PR
#16 tampoco lo bumpeo) -- si mergeas esto a `main` el deploy sale solo, asi que decidi
vos el numero antes. **Cero push, cero deploy, `git status` limpio.**
