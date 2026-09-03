# Los tres defectos que aparecieron midiendo, y que nadie habia pedido arreglar

**Fecha:** 2026-09-03
**Rama:** `0xultravioleta/x4-hallazgos` — **3 commits** sobre `origin/main` = `b528748a`
**Estado:** codigo listo, **sin pushear**. Cero deploys, cero terraform, cero docker push.
**Fuente:** `docs/handoffs/2026-09-02-paginas-listo.md` seccion 3, puntos 1, 2 y 3.
**Verificacion:** suite de CI en verde, clippy en **328 warnings y ninguno cae sobre
una linea que esta rama agrego** (759 lineas nuevas en 13 archivos, chequeado
linea por linea contra `git diff -U0`), y los tres arreglos comprobados contra el
binario corriendo, no solo contra los tests.

> **Ojo con la base:** el encargo decia que `main` era `1c4c33d9`. Cuando arranque,
> `origin/main` ya estaba en `b528748a` (`fix(alb)`). La rama sale de ahi. El PR #13
> con las ocho paginas sigue abierto y no se toco.

---

## 1. El resultado, en una linea cada uno

| # | que estaba mal | que hace ahora |
|---|----------------|----------------|
| 1 | un binario local podia **ganar** el writer lease de produccion y rutear los settles EVM a `127.0.0.1` | si la direccion que anunciaria es loopback o no se puede determinar, **no se presenta a la eleccion**, y lo dice en WARN |
| 2 | `escrow`, `commerce` y `upto` se anunciaban solo bajo CAIP-2: un cliente v1 leia "aca no hay escrow" | los cuatro esquemas pasan por **un unico paso** que los publica bajo las dos formas |
| 3 | `/supported` publicaba `base` y `eip155:8453` sin nada que los uniera | cada entrada lleva `networkAliases` con los dos identificadores |

---

## 2. Los tres commits

| # | commit | que |
|---|--------|-----|
| 1 | `357f9441` | **P0** el lease no se adquiere jamas anunciando loopback + `scripts/run-local.sh` |
| 2 | `ee6141e6` | **P1** escrow, commerce y upto tambien con nombre v1 |
| 3 | `d97af9bb` | **P2** `networkAliases`, aditivo |

15 archivos, +759 lineas bajo `src/`.

---

## 3. Defecto 1 (P0) — el binario local se metia en la eleccion de produccion

### Lo que pasaba

`writer_lease::from_env()` leia `NONCE_STORE_TABLE_NAME` con default
`facilitator-nonces` —la tabla **real**— y `aws_config::load_defaults()` toma las
credenciales del entorno sin preguntar. Corriendo el binario en local el 2026-09-02,
el proceso se presento a la eleccion del writer lease de **produccion**. La perdio
(el `ConditionalCheckFailed` dice que el lease no era suyo), pero pudo haberla ganado.

Ganarla no es un detalle de higiene. El ganador **publica su direccion en el
registro**, y todas las demas tareas le reenvian sus settles EVM. Un settle
reenviado a `127.0.0.1` de una laptop es un pago que nunca llega a la cadena.

### Que se hizo, y por que asi

El arreglo es de construccion, no un flag. `spawn()` (`src/writer_lease.rs`)
resuelve la direccion que anunciaria **antes** de construir el cliente de AWS, y si
es loopback (`127.0.0.0/8`, `::1`, `localhost` y `*.localhost`, `::ffff:127.0.0.1`),
la direccion no especificada (`0.0.0.0`, `::`) o no se puede determinar, **no emite
el PutItem condicional**. Loguea una linea WARN diciendo por que se abstiene y sigue
sirviendo todo lo demas, exactamente como con el lease apagado.

Tres decisiones que vale la pena dejar escritas:

1. **El chequeo corre antes del cliente de AWS.** Un proceso que no debe tocar la
   tabla del lease no la debe tocar **por ningun motivo**, ni para resolver
   credenciales contra ella.
2. **`WRITER_LEASE_ENDPOINT` sigue siendo la forma de declarar una direccion, pero
   deja de ser una forma de saltarse el chequeo.** El arreglo obvio de "lo pongo en
   `http://127.0.0.1:8080` y listo" reabriria el agujero, asi que hay un test que lo
   prohibe.
3. **Al abstenerse, el proceso sigue siendo writer** (`IS_WRITER` arranca en `true` y
   nadie lo baja). Es la misma postura fail-open que el modulo ya declara en la rama
   `Err`: preferir escrituras concurrentes —que `PendingNonceManager` sabe reintentar—
   antes que negar pagos. **En produccion esto cambia un caso:** una tarea que no
   pueda determinar su direccion ya no se presenta, asi que otra tarea —que si tiene
   direccion— gana el lease. Antes, si ganaba la que no tenia direccion, **todas las
   demas contestaban 503 en cada escritura EVM**. El cambio va del lado correcto.

Un hostname que no es un literal IP se acepta tal cual: un operador que apunta
`WRITER_LEASE_ENDPOINT` a un nombre DNS interno sabe lo que hace, y este proceso no
esta en posicion de discutirle al resolver del VPC.

### La verificacion, contra el binario vivo

Corrido **sin** `ENABLE_WRITER_LEASE` (o sea con el default ON, que es el caso
peligroso) y con credenciales AWS falsas:

```
WARN x402_rs::writer_lease: Not standing in the EVM writer lease election. Winning it
would route every other task's EVM settles to an address they cannot reach, so this
process abstains by construction rather than by kill-switch. It keeps serving every
route and keeps writing its own EVM transactions. Set WRITER_LEASE_ENDPOINT to an
address peers can reach in order to take part.
reason=this task could not determine an address other tasks can reach
```

Cero llamadas a DynamoDB. `/health` contesta `{"status":"healthy"}` igual que siempre.

### Los tests, y que son discriminantes en las DOS direcciones

Comprobado montando el estado malo: si se neutraliza el guard (que nunca se
abstenga) **3 tests se ponen en rojo**; si se lo exagera (que siempre se abstenga)
**otros 2 se ponen en rojo**. O sea que el guard queda clavado por arriba y por abajo.

- `loopback_addresses_refuse_the_election` — 10 formas de escribir una direccion que no sirve, la cadena vacia incluida
- `routable_addresses_still_stand_in_the_election` — una direccion routable elige igual que antes
- `explicit_loopback_override_cannot_skip_the_check` — el atajo no es un atajo
- `a_box_without_ecs_metadata_abstains_without_a_kill_switch` — el caso que realmente paso
- `explicit_kill_switch_is_unchanged_by_the_reachability_guard` — `ENABLE_WRITER_LEASE=false` no cambia de significado
- `endpoint_host_survives_every_shape` — `::1` era la trampa: partir por el ultimo `:` lo convierte en `:`, que no parsea como IP y pasaba de largo

### La receta local segura

Esta en **`scripts/run-local.sh`** (nuevo), y
`docs/handoffs/2026-09-02-superficies-agenticas-listo.md` ahora apunta ahi con el
porque escrito arriba. Elegi un script y no un parrafo de documentacion **porque la
leccion del defecto es exactamente esa**: lo que depende de que alguien se acuerde,
se olvida. El script:

- credenciales AWS falsas que **tapan** las reales del shell (`AWS_ACCESS_KEY_ID`,
  `AWS_SECRET_ACCESS_KEY`), `AWS_EC2_METADATA_DISABLED=true` para que no caiga a IMDS,
  y `AWS_PROFILE` deseteado para que no consulte un perfil con nombre
- `NONCE_STORE_TABLE_NAME`, `TRANSACTIONS_TABLE_NAME`, `IDEMPOTENCY_TABLE_NAME` y las
  dos de DX402 **deseteadas**
- `ENABLE_WRITER_LEASE=false` como cinturon ademas de los tirantes del codigo
- una clave de firma efimera, de un solo uso, sin fondos y que nunca se escribe a disco

```bash
PORT=8402 ./scripts/run-local.sh
```

### Una asimetria que quedo a la vista y NO se toco

`nonce_store::create_nonce_store()` (`src/nonce_store.rs:407`) usa memoria si
`NONCE_STORE_TABLE_NAME` **no esta seteada**. `writer_lease::from_env()`
(`src/writer_lease.rs`) hace lo contrario: si no esta seteada, cae al nombre de la
tabla de produccion. Eso era la mitad del problema. El guard de direccion ya deja el
caso local cerrado sin tocar ese default, y cambiarlo es un cambio de comportamiento
aparte que no estaba en el encargo. Queda anotado.

---

## 4. Defecto 2 (P1) — el contrato que mentia por omision

### La medicion, antes

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | python3 -c 'import sys,json,collections
k=json.load(sys.stdin)["kinds"]; c=collections.Counter()
for e in k: c[(e["scheme"], "caip2" if ":" in e["network"] else "v1")]+=1
print("total", len(k))
for s in sorted({s for s,_ in c}): print(f"  {s:12} v1={c[(s,\"v1\")]:>3}  caip2={c[(s,\"caip2\")]:>3}")'
```

| esquema | nombre v1 | CAIP-2 |
|---------|-----------|--------|
| `exact` | **38** | 38 |
| `escrow` | **0** | 14 |
| `commerce` | **0** | 11 |
| `upto` | **0** | 11 |
| `fhe-transfer` | 1 | 1 |
| **total** | | **114 kinds** |

Un cliente que descubre esquemas leyendo las entradas v1 —o sea, **toda integracion
escrita antes de que CAIP-2 existiera**— concluia que este facilitador no tiene
escrow.

### La causa no estaba en los tres esquemas

Estaba en **donde** se hacia el espejo. El bloque que duplicaba cada entrada en
CAIP-2 corria como su propio paso **antes** de que escrow, commerce y upto se
pushearan. Todo lo empujado despues quedaba afuera por construccion, y nadie que
agregara un esquema nuevo tenia forma de enterarse.

Ahora el espejo es **un unico paso sobre la lista terminada**
(`advertise_under_both_network_forms`, `src/facilitator_local.rs`) y los cuatro
esquemas pasan por el sin duplicar codigo. Un esquema que se agregue manana sale
bajo las dos formas sin que su autor tenga que saber que hay dos formas.

Tres cosas que salieron de mover el paso al final:

- **FHE deja de tipear `eip155:11155111` a mano.** El espejo lo deriva de
  `Network::to_caip2()`, asi que ya no pueden divergir.
- **`upto` estaba acoplado a que el espejo corriera primero.** Derivaba su lista
  filtrando `k.network.starts_with("eip155:")`, o sea que solo veia las entradas
  espejadas. Ahora resuelve por el enum, que es lo que siempre quiso decir.
- **La identidad para deduplicar incluye `extra`**, porque `escrow` publica una
  entrada por PaymentOperator desplegado en la misma cadena y no se distinguen en
  nada mas. Una clave sin `extra` se habria comido todos los operadores menos uno.

### El conteo nuevo, dicho antes de que sorprenda

**`/supported` pasa de 114 a 150 kinds.** Son exactamente los 36 gemelos v1 que
faltaban: escrow 14 + commerce 11 + upto 11. **Ninguna entrada existente cambia de
forma ni de nombre**; solo se agregan. Los 36 identificadores CAIP-2 involucrados
(15 distintos: `eip155:1`, `:10`, `:56`, `:137`, `:143`, `:999`, `:8453`, `:42161`,
`:42220`, `:43113`, `:43114`, `:84532`, `:421614`, `:11155111`, `:1187947933`) los
resuelve el enum sin excepcion, asi que la proyeccion es exacta y no una estimacion.

Medido contra el binario local con `ENABLE_PAYMENT_OPERATOR=true ENABLE_UPTO=true` y
tres RPC EVM:

| esquema | nombre v1 | CAIP-2 |
|---------|-----------|--------|
| `exact` | 5 | 5 |
| `escrow` | **14** | 14 |
| `commerce` | **11** | 11 |
| `upto` | **3** | 3 |
| `fhe-transfer` | 1 | 1 |

### Un efecto colateral que conviene saber: `/accepts` tambien mentia

`post_accepts` (`src/handlers.rs`) arma un lookup `(scheme, network) -> extra` a
partir de `supported().kinds` y **descarta en silencio** cualquier requirement que no
matchee. O sea que un comercio que mandaba `{"scheme":"escrow","network":"base"}`
—con el nombre v1— recibia su requirement descartado sin explicacion. Ahora matchea.
No toque `post_accepts`: se arreglo solo al arreglar la fuente.

(Anotado aparte, **no** tocado: ese lookup pisa la clave repetida, asi que cuando hay
varios PaymentOperator en una cadena solo sobrevive el `extra` del ultimo. Es
comportamiento preexistente, ya pasaba con las entradas CAIP-2, y arreglarlo es
decidir cual operador es el canonico — otro encargo.)

---

## 5. Defecto 3 (P2) — las dos formas ahora se nombran entre si

Cada entrada lleva `networkAliases` con **todos** los identificadores que nombran esa
misma cadena, el suyo incluido:

```json
{
  "x402Version": 1,
  "scheme": "escrow",
  "network": "base-sepolia",
  "networkAliases": ["base-sepolia", "eip155:84532"],
  "extra": { "escrowAddress": "0x2902...", "operatorAddress": "0x7d09...", "tokenCollector": "0x5ca7..." }
}
```

Se llena en el **mismo unico paso** que ya resolvio la cadena de cada entrada, asi
que no hay una segunda fuente de verdad que pueda derivar. `Network` no tenia un
nombre para este concepto, asi que use el que propuso el dueno.

**Aditivo de verdad:** es `Option`, **desaparece del JSON** cuando la red no se puede
resolver —decir nada es mejor que inventar un identificador— y ningun campo existente
cambia de forma ni de nombre. Un cliente que lo ignore lee exactamente lo que leia
antes.

### El hallazgo que solo se vio corriendo el binario

`get_supported` **no serializa el struct v1**: convierte antes por
`SupportedPaymentKindsResponseV1ToV2::to_v2` (`src/types_v2.rs`). La primera version
calculaba `networkAliases` bien y lo tiraba ahi. **Los cinco tests unitarios pasaban
todos**; el `curl` contra el binario vivo mostro **0 de 68 entradas** con el campo.
Por eso `SupportedPaymentKindV2` lo lleva tambien, y hay un test
(`the_alias_survives_the_conversion_that_reaches_the_wire`) que ata el campo a la
conversion que efectivamente llega al cable, no al struct interno.

Es la misma leccion que las tres del encargo: ninguno de estos defectos se veia
leyendo el codigo.

Despues del arreglo, contra el binario vivo: **68 de 68 entradas con `networkAliases`**.

### Redes sin mapeo: NINGUNA

El test `every_supported_network_resolves_from_both_of_its_names` recorre las **39**
redes de `Network::variants()` y comprueba que el nombre v1 y el CAIP-2 se resuelven
mutuamente. Pasan las 39. Si alguna fallara, el test la **nombra** en el mensaje en
vez de saltearla.

Las tres que el enum declara pero produccion no sirve —`sei`, `sei-testnet`, `xdc`—
tampoco tienen problema: verificado a mano, resuelven en las dos direcciones. No
entran en `variants()` y por eso no las cubre el test.

### Lo que NO se hizo, y por que

**No se agregaron los tokens que faltan a las 49 entradas CAIP-2.** Es otro encargo.
Pero midiendolas quedo claro que **el problema no es de CAIP-2**, asi que la proxima
sesion no tiene que empezar de cero:

| cuantas | quienes | por que no traen `tokens` |
|---------|---------|---------------------------|
| 36 | `escrow` 14 + `commerce` 11 + `upto` 11 | por diseno: escrow y commerce llevan las direcciones del escrow en `extra`, `upto` no lleva `extra` |
| 1 | `fhe-transfer` | se proxea al Lambda de Zama, que maneja sus propios tokens |
| 12 | `exact` en **algorand, fogo, near, solana, stellar y sui** (mainnet y testnet) | esos providers publican `feePayer` pero **no** una lista de tokens |

Las mismas 12 aparecen del lado v1 (13 entradas v1 sin tokens = esas 12 + el
`fhe-transfer` v1). O sea: **no falta nada del lado CAIP-2 que no falte igual del
lado v1**. Lo que hay que medir es por que los seis providers no-EVM no publican
tokens, y eso se responde en `src/chain/{solana,near,stellar,sui,algorand}.rs`, no en
`/supported`.

---

## 6. Los curl de verificacion

```bash
# 1) el conteo por esquema y por forma de nombrar la cadena (114 -> 150)
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds | length'
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -r '.kinds[] | "\(.scheme)\t\(if (.network|test(":")) then "caip2" else "v1" end)"' \
  | sort | uniq -c

# 2) escrow tiene que aparecer con nombre v1 (antes: cero resultados)
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq '[.kinds[] | select(.scheme=="escrow" and (.network|test(":")|not))] | length'

# 3) todas las entradas tienen que traer networkAliases
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq '[.kinds[] | select(.networkAliases == null)] | length'   # -> 0

# 4) y los alias tienen que apuntar a entradas que existen
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -r '.kinds[] | select(.scheme=="exact") | "\(.network) -> \(.networkAliases|join(", "))"' | head

# 5) local, con la receta segura (el WARN del lease tiene que salir si se quita
#    ENABLE_WRITER_LEASE=false del script)
PORT=8402 ./scripts/run-local.sh
curl -s http://127.0.0.1:8402/supported | jq '.kinds | length'
```

---

## 7. Como quedo verificado

```bash
cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
cargo clippy -p x402-rs --features solana,near,stellar,algorand,sui,xrpl --all-targets
# los crates del workspace, que dependen de x402-rs por path y por eso ven el
# campo nuevo de SupportedPaymentKind:
cargo clippy -p x402-compliance && cargo test -p x402-compliance -- --test-threads=1
cargo check -p x402-axum -p x402-reqwest
```

- **suite en verde**: 770 tests de lib + 740 de bin + 19 de integracion + doc-tests,
  cero fallos. Son **17 tests nuevos** (6 del lease, 6 del anuncio, 5 del alias).
- **clippy en 328 warnings**, y **ninguno cae sobre una linea que esta rama agrego**:
  chequeado cruzando cada `--> src/archivo:linea` del log contra las 759 lineas
  agregadas segun `git diff -U0`. Los dos warnings que apuntan a
  `facilitator_local.rs` (`ExactPaymentPayload` sin usar, `provider_map` sin usar) ya
  existian en `HEAD`.
- **los crates del workspace compilan**: `x402-axum` y `x402-reqwest` dependen de
  `x402-rs` por path, asi que el campo nuevo de `SupportedPaymentKind` los alcanza —
  `cargo check` limpio en los dos, y `x402-compliance` con clippy limpio y sus 10
  tests en verde.
- los tres arreglos comprobados **contra el binario corriendo**, no solo contra los
  tests, que es de donde salieron los tres defectos en primer lugar.

---

## 8. Lo que queda anotado para otra sesion

1. **Los tokens de las 49 entradas** — medicion inicial en la seccion 5. El trabajo
   real esta en los seis providers no-EVM, no en `/supported`.
2. **El default de `NONCE_STORE_TABLE_NAME` en `writer_lease`** apunta a la tabla de
   produccion mientras `nonce_store` cae a memoria. Seccion 3.
3. **`post_accepts` pisa la clave repetida** y solo conserva el `extra` del ultimo
   PaymentOperator por cadena. Preexistente. Seccion 4.
4. **La trampa del worktree sigue viva** y le va a pasar al proximo: el `.git` del
   worktree apunta a `Z:/...` y el git de WSL no lo resuelve. Ademas **todo el arbol
   aparece modificado** porque el checkout lo hizo el git de Windows con CRLF y el de
   WSL lee los blobs en LF. Todos los comandos de esta sesion corrieron con
   `git -c core.autocrlf=input`, que deja el `status` limpio sin tocar la
   configuracion compartida del checkout del dueno.
