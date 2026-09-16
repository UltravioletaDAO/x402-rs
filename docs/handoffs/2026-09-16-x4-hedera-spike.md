---
date: 2026-09-16
tags:
  - type/handoff
  - domain/hedera
  - priority/p0
status: active
---

# Hedera nativo, fase 0: el SDK de Rust conserva los bytes

**VEREDICTO: GO.** `hiero-sdk 0.45.0`, el crate **publicado**, decodifica un
payload real de `@x402/hedera 2.26.0`, lo cofirma y lo reserializa dejando los
cuerpos y las firmas previas **idénticos byte a byte** en las **siete** variantes
por nodo que emite el cliente oficial, con Ed25519, ECDSA secp256k1, KeyList y
umbral 2 de 3 — 15 aserciones verdes en `tests/hedera-e2e/rust-spike`, corrida
del 2026-09-16, con `protoc` como única dependencia de sistema nueva.

No es lectura de código. Es `cargo test`, y la salida completa está en
`tests/hedera-e2e/RESULTS-2026-09-16.txt`.

## El experimento

Seis pasos por vector, y el sexto es la pregunta entera:

1. decodificar el base64 que enviaría un pagador real;
2. inspeccionar **todas** las variantes por nodo en protobuf, no la primera;
3. decodificar con `hiero-sdk` y contrastar lo que el SDK dice;
4. verificar la firma del pagador sobre los cuerpos congelados;
5. cofirmar como patrocinador;
6. reserializar, volver a decodificar y comparar **byte a byte**.

Los vectores los produce el lado TypeScript porque es el lado que corre un
pagador de verdad: `@x402/hedera` construye la transferencia, la congela contra
`Client.forTestnet()` y la firma, y ese base64 es literalmente lo que llega en
`payload.transaction`. Un vector generado por el SDK de Rust y leído por el SDK
de Rust no habría demostrado nada sobre interoperabilidad.

### Lo que contestó

| Propiedad | Resultado |
|---|---|
| El crate publicado decodifica el payload del cliente oficial | sí, y ve las **7** variantes, no la primera |
| Lo que lee Rust en protobuf == lo que escribió TypeScript | idéntico en los 15 vectores: cuerpos, longitudes, ids, prefijos y firmas |
| `to_bytes` sin firmar nada reproduce la entrada | **byte a byte idéntica** — nada se reconstruye desde campos parseados |
| Cofirma: cuerpos intactos | sí, en todas las variantes |
| Cofirma: firmas previas intactas | sí |
| Cofirma: una firma nueva por variante, y verifica | sí |
| Ed25519 / ECDSA secp256k1 / KeyList / umbral 2 de 3 | los cuatro verifican; el umbral pasa con dos firmas y no exige la tercera |
| ¿Hace falta `protoc`? | **sí**, y el build falla sin él |

La preservación no es casualidad del camino feliz: es estructural.
`Transaction::to_bytes` devuelve `signed_sources().transactions()` tal cual
cuando la transacción vino de `from_bytes`, y `TransactionSources::sign_with`
sólo empuja un `sigPair` al `sig_map` de cada variante — `body_bytes` no se toca
en ningún punto del recorrido.

## Versiones medidas el día de la corrida, no copiadas del plan

| Cosa | Valor medido 2026-09-16 | Nota |
|---|---|---|
| npm `@x402/hedera` | **2.26.0** | el plan decía 2.25.0. Se movió, como estaba avisado. Deps: `@x402/core ~2.26.0`, `@hiero-ledger/sdk 2.85.0`, `@hiero-ledger/proto 2.31.0` — los SDK no se movieron |
| crates.io `hiero-sdk` | **0.45.0**, publicado 2026-04-30 | sigue siendo la máxima publicada |
| GitHub `hiero-sdk-rust` | tag `v0.46.0` = `3b0696e8…` | **existe y no está publicado**. No hizo falta: no hay razón para una dependencia por git |
| crates.io `hedera` (nombre viejo) | 0.43.0 | no se usa |
| `hiero-sdk-proto` | **0.22.0** (lo que resuelve `hiero-sdk 0.45.0`) | `hedera-proto` 0.20.0 es el nombre viejo, tampoco se usa |

El crate publicado bastó. **No hay que meter una dependencia por git en un
binario que mueve dinero**, que era la decisión reversible que el encargo dejaba
abierta.

## `protoc`: sí, y en dos sitios, no tres

`hiero-sdk` arrastra `hiero-sdk-proto 0.22.0`, cuyo `build.rs` genera el código
con `tonic-build 0.12` y **no trae compilador vendorizado**. Sin `protoc` en el
PATH, desde un `target/` limpio:

```
error: failed to run custom build command for `hiero-sdk-proto v0.22.0`
  Error: Could not find `protoc`. If `protoc` is installed, try setting the
  `PROTOC` environment variable to the path of the `protoc` binary. ...
```

Con `protoc` presente compila y los tests pasan. Dónde hay que añadirlo:

1. **`Dockerfile`, etapa builder** (`apt-get install` de `pkg-config` y
   `libssl-dev`): `protobuf-compiler`.
2. **`.github/workflows/ci.yaml`, paso «Install build dependencies»**: la imagen
   `ubuntu-24.04` a la que resuelve `ubuntu-latest` documenta `cmake`, `jq`,
   `curl` y `pkg-config`, y **cero coincidencias** de `protobuf`/`protoc`. Hoy el
   repositorio ya usa `tonic` y `prost` y compila sin `protoc` porque **no tiene
   `prost-build`** en el lock; en cuanto entre `hiero-sdk-proto`, lo tendrá.

**Y no en la etapa de runtime**, aunque el `Dockerfile` tenga dos etapas y la
tentación sea tocar las dos: la etapa 2 es `debian:bookworm-slim`, instala `ca-certificates` y `curl`, y copia un
binario ya enlazado. Ahí `protoc` sería peso muerto. Refutado, medido en el
propio `Dockerfile`.

`openssl` no es un problema: `hiero-sdk` usa `openssl`/`hyper-openssl`, y el
facilitador **ya** enlaza `openssl`. `libssl-dev` ya está en el builder.

### Lo que cuesta

| Medida | Valor |
|---|---|
| Crates nuevos en el árbol | **15**: `arc-swap`, `backoff`, `fraction`, `hiero-sdk`, `hiero-sdk-proto`, `hybrid-array`, `hyper-openssl`, `linked_hash_set`, `md5`, `pkcs5`, `prost-types`, `salsa20`, `scrypt`, `triomphe`, `unsize` |
| Sobre un lock de | 1089 nombres |
| Build limpio del spike, debug | 22,6 s |
| Build limpio del spike, release | 52,6 s |

Medido en esta máquina con `CARGO_BUILD_JOBS=4` y sin `sccache`. Es el coste del
subárbol de Hedera aislado, no el delta exacto sobre la imagen de producción: eso
se mide cuando la fase 1 declare la dependencia de verdad. La feature debe quedar **fuera**
de `CARGO_FEATURES` del CI y de las dos líneas del Dockerfile hasta entonces.

## La línea divisoria, que es el resultado que importa para la fase 2

De los diez vectores adversariales, **el SDK rechaza cuatro y cofirma seis**.

`AnyTransaction::from_bytes` sí mira todas las variantes: compara los cuerpos con
`pb_transaction_body_eq` y rechaza la lista si discrepan. Cayeron ahí:

| Vector | Mensaje del SDK |
|---|---|
| 06 variante que paga a otra cuenta, **con firma válida** | `transaction parts unexpectedly unequal` |
| 07 variante con importe x1000 y firma vieja | `transaction parts unexpectedly unequal` |
| 09 variante con **otro transaction id** | `chunks in non chunkable transaction` |
| 10 `pubKeyPrefix` vacío | `Transaction has mismatched signatures` |

Los otros cinco son transferencias de Hedera perfectamente válidas. El SDK las
decodifica, el pagador las firmó de verdad, y el SDK **las cofirma sin decir
nada**:

| Vector | Qué es | Quién lo para |
|---|---|---|
| 11 `isApproval=true` en el débito | gasto de allowance, no del pagador | sólo nosotros |
| 12 NFT de polizón junto al USDC | transferencia lateral | sólo nosotros |
| 13 `preTxAllowanceHook` sobre el débito | un contrato corre antes | sólo nosotros |
| 14 dos variantes con el mismo nodo | dos copias enviables | sólo nosotros **en Rust** |
| 15 el patrocinador pone el 40% del principal | nos cobra a nosotros | nosotros y la referencia |

Dos consecuencias que hay que llevarse enteras:

**Los getters agregados del SDK no pueden expresar esto.**
`TransferTransaction::get_hbar_transfers()` devuelve `HashMap<AccountId, Hbar>` y
`get_token_transfers()` devuelve `HashMap<TokenId, HashMap<AccountId, i64>>`. En
ninguno de los dos cabe `isApproval`, ni un `hookCall`, ni una entrada duplicada;
el `struct Transfer` que sí los tiene es `pub(crate)`. La inspección tiene que
leer protobuf con `hiero-sdk-proto`, que es una dependencia declarada y fijada, y
no delegarse en los getters. `rust-spike/src/policy.rs` es la prueba de que las
reglas 2, 4, 5 y 6 de la §7.3 del plan se expresan sobre los bytes que llegan.

**Y el facilitador de referencia tiene el mismo punto ciego.** Corrida sobre los
mismos vectores, `@x402/hedera 2.26.0` **acepta** 11, 12 y 13. Su
`inspectHederaTransaction` lee `transaction.hbarTransfers` /
`transaction.tokenTransfers`, que son los getters agregados del SDK de
TypeScript. La firma del pagador y el preflight se sustituyeron por `ok` para
correr sin red: no es un favor, el pagador firmó esos cuerpos de verdad y el
preflight sólo recibe `{payer, payTo, asset, amount, network}` — nunca ve la
transacción, así que no podría haber atrapado ni el NFT ni el hook. **No copiar
la inspección de la referencia.**

### Una divergencia entre los dos SDK, medida

El vector **14** (dos variantes nombrando el nodo `0.0.3`) lo **rechaza**
`Transaction.fromBytes` de TypeScript y lo **acepta** `AnyTransaction::from_bytes`
de Rust, que además reporta tres node ids con uno repetido. Es una diferencia
real de comportamiento entre las dos implementaciones sobre los mismos bytes: la
deduplicación de nodos es nuestra, no del SDK.

## Trampas del SDK que la fase 2 debe respetar

- **`PublicKey::verify_transaction` cortocircuita sobre los firmantes propios.**
  Su primera línea es: si la clave está entre los `signers` de la transacción,
  devuelve `Ok(())` sin verificar nada. Verificar **siempre antes** de cofirmar, y
  nunca «verificar» una clave que nosotros mismos añadimos.
- **`pub_key_prefix` se compara con `starts_with`, y el prefijo vacío es prefijo
  de todo.** En `sign_with` eso hace que el SDK se **salte** nuestra firma
  creyendo que ya está; en `verify_transaction` obliga a una verificación que no
  se pidió. 0.45.0 rechaza esas listas al decodificar, pero eso es una propiedad
  de **esta** versión: la política lo rechaza por su cuenta y hay un test que lo
  fija.
- **`pb_transaction_body_eq` ignora `transaction_id`, `node_account_id`,
  `batch_key` y `high_volume`.** El vector 09 acabó rechazado por la lógica de
  chunks, no por la comparación, y con un mensaje que no describe el problema. No
  apoyarse en esa comparación: comparar el transaction id nosotros.
- **`PrivateKey.fromSeedED25519` (TS) y `PrivateKey::from_bytes_ed25519` (Rust)
  no son la misma función.** La primera deriva con HMAC-SHA512; la segunda toma
  los 32 bytes como clave. Los mismos «bytes de semilla» producen **cuentas
  distintas**. Lo encontró un test del spike, no una lectura; queda fijado en
  `both_sdks_derive_the_same_keys_from_the_same_labels`.

## Contrato de wire, cerrado con la referencia en la mano

Medido sobre `@x402/hedera 2.26.0`, no sobre la especificación:

- **`settle` devuelve `{ success, network, payer, transaction }`**, donde
  `transaction` lleva el **transaction id** de Hedera (`0.0.x@segundos.nanos`) y
  `payer` lleva el **remitente real**, con el patrocinador sólo como respaldo
  cuando el remitente no se pudo inferir. Se adopta el contrato del SDK, como
  proponía el plan. El molde en este repositorio es `TransactionHash::Algorand(String)`
  (`src/types.rs:1284`): un identificador en texto, no 32 bytes.
- **`getExtra` elige el `feePayer` al azar** entre las direcciones del signer, y
  `getSigners` las devuelve todas, bajo `caipFamily = "hedera:*"`.
- **`accepted` y `paymentRequirements` deben coincidir** en asset, amount, payTo,
  maxTimeoutSeconds y `extra.feePayer`, o la referencia responde
  `accepted_payment_requirements_mismatch` antes de mirar la transacción. Es
  exactamente la información que `PaymentPayloadV2::to_v1()` pierde hoy.
- `aliasPolicy` por defecto es `"reject"`: un `payTo` con alias no se acepta.

## Qué NO se midió

- **Nada tocó la red de Hedera.** Ni nodo de consenso, ni Mirror Node, ni fondos,
  ni cuentas reales, ni mainnet. `execute()`, el recibo de consenso,
  `DUPLICATE_TRANSACTION` y la resolución de la clave de cuenta por Mirror Node
  siguen sin ejecutarse: son de la fase 4, y el plan ya las lista.
- La firma del pagador en la corrida de la referencia está **stubbeada**; en la
  corrida de Rust es real, sobre los cuerpos congelados.
- El vector 01 no es reproducible byte a byte: su transaction id y su juego de
  nodos los elige el SDK. Los otros catorce sí, con valid-start fijo.
- El delta de build sobre la imagen de producción no está medido; lo del spike
  aislado, sí.

## Qué implica para las fases 1 a 5

- **Fase 0: cerrada, y la rama alternativa queda descartada.** No hace falta ni
  codec protobuf propio ni servicio auxiliar en TypeScript. La comparación de
  coste que correspondía al caso NO-GO **no aplica** y no se redacta: el
  camino recomendado por el plan es el que funciona. La ruta EVM de Hedera sigue
  fuera de discusión como reemplazo del esquema nativo.
- **Fase 1** (tipos y routing) puede arrancar tal como está planificada. Lo que
  este spike le añade: el payload Hedera se elige leyendo `network`, no por forma;
  el tipo validado de entity ID es obligatorio (`0.0.0` es el activo de HBAR y
  hoy la regex de NEAR en `MixedAddress` se lo come); Hedera emite sólo v2; la
  feature de cargo queda fuera de `CARGO_FEATURES` y del Dockerfile hasta que la
  fase 2 esté verde.
- **Fase 2** (verificación) es donde va el peso, y ahora se sabe cuánto: el SDK
  cubre la igualdad entre variantes y la consistencia del mapa de firmas, y **no**
  cubre riders, allowances, hooks, nodos duplicados ni la política de importes.
  `tests/hedera-e2e/rust-spike/src/policy.rs` es el borrador ejecutable de esas
  reglas y los quince vectores son su matriz de regresión. Se puede portar casi
  literal a `src/chain/hedera/verify.rs`; el módulo `codec` debe leer protobuf.
- **Fase 3** (liquidación durable) no se movió: el spike no ejecuta nada contra la
  red, así que el diseño de reservas, recibo y estado incierto sigue entero y sin
  evidencia nueva.
- **Fases 4 y 5** tampoco. La condición de arranque sigue siendo cuentas propias
  con HBAR y asociaciones HTS previas, en cada red.
- **La estimación de 14–21 días no cambia** por la fase 0: se confirma el extremo
  bueno de la horquilla, porque la decisión que podía sumar «+1 a 3 semanas» ya no
  existe. La fase 2 no se abarata: el trabajo que el SDK no hace está ahora
  enumerado, y son seis clases de rechazo con nombre propio.

## Reproducir

```bash
cd tests/hedera-e2e
npm install && node generate-vectors.mjs        # vectores, sin red y sin fondos
node reference-verify.mjs                  # el facilitador de referencia, mismos vectores
cd rust-spike && cargo test                # 15 aserciones: el veredicto
cargo run                                  # el informe legible
```

`protoc` es obligatorio para el paso de Rust: `brew install protobuf` o
`apt-get install -y protobuf-compiler`.

## Comprobaciones locales antes del push

| Job del CI | Comando | Resultado |
|---|---|---|
| portada offline | `python3 scripts/verify_landing_canonical.py --offline` | OK, `hedera references: 0` |
| build | `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl` | OK |
| tests facilitador | `cargo test --locked -p x402-rs --features … -- --test-threads=1` | OK |
| tests crates | `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | OK |
| spike | `cargo test` en `tests/hedera-e2e/rust-spike` | 15/15 |

Este cambio no toca `src/`, ni `Cargo.toml`, ni `Cargo.lock`, ni `static/`, ni
`terraform/`, ni `VERSION`. Añade `tests/hedera-e2e/` y este handoff, y nada más.
El crate del spike tiene su propio `[workspace]` para no entrar en el del
facilitador; su `cargo build` emite tres avisos de `patch … was not used` porque
hereda el `[patch.crates-io]` de `.cargo/config.toml`, y son inofensivos.
