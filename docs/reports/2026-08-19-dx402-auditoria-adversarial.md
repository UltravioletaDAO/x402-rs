# Auditoría adversarial de DX402 — resultados y estado

**Fecha:** 2026-08-19 (corrida nocturna sin supervisión)
**Alcance:** nuestra fuente dorada — `src/dx402/`, `src/erc8004/proof.rs`,
`crates/x402-{axum,reqwest}`, el Terraform de DX402, y los SDKs py/ts.

---

## 0. Advertencia sobre el reporte crudo del equipo

El workflow reportó **"46 hallazgos, 46 sobrevivieron, 0 refutados"**. Eso es un
**artefacto, no un resultado**.

De 72 agentes, 64 murieron por límite de uso — y los 64 eran **todos los
verificadores adversariales**. Con cero votos, el script clasificó cada hallazgo
como "no refutado" por defecto. Sobrevivieron 8 agentes: los que *encuentran*,
ninguno de los que *refuta*.

O sea que había **46 afirmaciones sin verificar**, no 46 defectos confirmados. La
mitad adversarial del diseño —la que existe justamente para que no reportemos
como real algo plausible pero falso— no llegó a correr. Todo lo que sigue lo
verifiqué a mano contra el código.

**Los agentes tenían permiso de escritura.** Les pedí un reporte y varios
editaron el árbol de trabajo. Uno de esos cambios resultó mejor que mi propio
diseño (el modelo de tres rangos, §1) y lo adopté tras revisarlo; el resto lo
revisé línea por línea antes de aceptar nada. Para la próxima: agentes de
auditoría con `Explore` o equivalente de solo lectura.

---

## 1. CRÍTICO — la finalidad era autodeclarada

`verified` es el flag que vuelve un registro **final**: nada puede reemplazarlo.
Se calculaba contra `req.payee`, **un campo que manda el llamador**.

Probar "controlo la dirección que tecleé en mi propio request" alcanzaba. Y el
`paymentId` es `keccak256(caip2 || txHash)` sobre datos públicos, así que
cualquiera que observe un settlement lo calcula.

Peor todavía: era una **regresión de mi propio arreglo de v1.82.0**. Ese arreglo
introdujo provisional/verificado para cerrar el hijack — pero dejó un camino
directo a FINAL, y el reclamo autofirmado no solo ganaba el turno, además
**desplazaba** el registro del vendedor real. Antes de v1.82.0 el atacante ganaba
solo si llegaba primero; después, ganaba siempre.

**Arreglo:** tres rangos en vez de un flag (`EvidenceRecord::authority`):

| rango | significa | quién lo otorga |
|---|---|---|
| 2 | la cadena dice que este es el payee | solo el gate |
| 1 | el reclamante se comprometió con una identidad | firma sobre el payee declarado |
| 0 | cualquiera pudo escribir esto | nadie |

Solo un rango **estrictamente mayor** desplaza. Rangos iguales son reclamos
indistinguibles y ahí manda el anti-replay: gana quien escribió primero. Eso
conserva las dos propiedades: el autofirmado deja de ser final, y el vendedor
legítimo sigue superando a un okupa anónimo sin necesitar RPC.

## 2. CRÍTICO — un cuerpo pagado se entregaba VACÍO

`settle_after_execution` liquida **antes** del hook. Si el cuerpo excedía
`max_body_bytes`, `attach_durable_evidence` devolvía `Body::empty()`: el
comprador pagaba, el nonce quedaba gastado, y recibía un `200` sin bytes.
Irrecuperable.

Viola la regla más importante de DX402: **degrada, nunca retiene**. "Sin
evidencia" nunca puede convertirse en "sin mercadería".

**Arreglo:** `buffer_body` devuelve el cuerpo junto al motivo del skip y la
respuesta se rearma siempre con los bytes reales. De paso se cerró el OOM
asociado: `collect()` bufferaba el cuerpo entero para *recién después* medirlo,
así que una descarga de varios GB tumbaba la task de 2 GB en vez de saltearse.
Ahora se consulta `size_hint()` primero y un cuerpo que se declara grande pasa de
largo sin tocarse.

## 3. ALTO — un duplicado destruía la evidencia que perdía

El blob se subía a S3 **antes** del control de anti-replay. `put_object` es
incondicional, el bucket tiene versionado deliberadamente apagado (una promesa de
retención que conserva versiones no es una promesa de retención), y la llave sale
del `paymentId` público.

Resultado: cualquiera mandaba un `paymentId` ya anclado con basura, destruía el
ciphertext real de forma irreversible, y recibía un `409` prolijo como si nada
hubiera pasado. El `contentHash` registrado ya no se podía reproducir nunca, y la
etiqueta de retención quedaba reescrita.

**Arreglo:** se reserva el turno en el registro primero; solo quien lo ganó
escribe bytes.

## 4. ALTO — la condición de DynamoDB no hacía nada contra la tabla real

La expresión condicional solo miraba `verified = :f`. En DynamoDB una comparación
contra un atributo **ausente** es falsa — y todo registro escrito antes de que el
flag existiera carece de él. Así que se negaba a desplazar justamente a los
registros viejos, que son los que más lo necesitaban.

Leía como funcionando en los tests en memoria y no hacía nada contra producción.

**Arreglo:** `attribute_not_exists(verified) OR verified = :f`, y lo mismo para
`signed`.

## 5. ALTO — un rechazo definitivo disfrazado de veredicto ausente

`ProofRejection::NotEvmTransaction` se mapeaba a `UnverifiableChain`, que por
diseño nunca bloquea. Pero su propia definición dice *"this one DOES refuse"*, y
solo se llega a ese mapeo tras resolver un proveedor EVM. En fase 2 una prueba
definitivamente mala habría pasado igual.

Es el error ya documentado para el gate de ERC-8004, repetido.

## 6. ALTO — las testnets firmaban la forma equivocada del digest

El bug de 0.53.0 seguía vivo. `_seller_digest_for` / `sellerDigestFor` caían a la
forma ed25519 cuando el payee **era EVM** pero el chain id no se resolvía. Eso no
levanta nada: produce una firma que nunca verifica, el anchor queda provisional
para siempre y nadie se entera.

**Medido:** la tabla de redes de los SDKs no tiene chain id para **ninguna
testnet EVM** — `base-sepolia`, `avalanche-fuji`, `polygon-amoy`, `xdc`, `sei`
devuelven `None`. Todo vendedor en base-sepolia estaba firmando mal, justo donde
se prueba.

**Arreglo:** leer el chain id directo de un CAIP-2 `eip155:N` (está ahí literal,
no hace falta la tabla), y si aun así no se resuelve, **no firmar** y reportar
`unsigned: "unknown_chain_id"`. Sin firmar es honesto y recuperable;
firmado-pero-inútil solo aparenta estar hecho.

## 7. ALTO — hex permisivo en TypeScript

El decodificador a mano usaba `parseInt`, que devuelve `NaN` ante entrada no-hex
— y un `Uint8Array` guarda `NaN` como `0`. Así que `'zz'.repeat(32)` decodificaba
a 32 bytes en cero y `parseInt('4z', 16)` a `4`.

Custodia material de clave real: la privada del pagador en `recoverEvidence`, la
firma de 65 bytes en `payerKeyFromEvmSignature`, la dirección del payee en
`anchorDigest`. Misma clase que el `rjust(32)` de base58 en Python. Python y Rust
ya rechazaban; TypeScript era el permisivo del trío.

**Arreglo:** delegar en `@noble/hashes/utils`, ya dependencia directa e importada
en ese mismo archivo.

## 8. MEDIO — base32 a mano aceptaba direcciones no canónicas

Una dirección de Algorand son 58 caracteres = 290 bits para 36 bytes (288 bits):
2 bits de relleno. El decoder los descartaba **sin verificar que fueran cero**,
así que el mapeo dirección→clave no era inyectivo y se aceptaban cadenas que no
son la codificación canónica de ninguna dirección.

**Arreglo:** `data_encoding::BASE32_NOPAD`, que ya estaba en el árbol como
dependencia transitiva de siete crates. Rechaza un **superconjunto estricto** de
lo que rechazábamos — la única dirección en la que puede moverse un decoder que
custodia claves.

---

## Un patrón que apareció tres veces

Tres tests **fijaban un bug como comportamiento esperado**:

- `test_unknown_network_falls_back_instead_of_crashing` (py)
- `falls back instead of throwing on an unknown network` (ts)
- `non_evm_proof_rejections_map_to_unverifiable_chain` (rust)

Los tres tenían una intención legítima ("que no explote", "que no bloquee") y la
codificaron afirmando el comportamiento equivocado. Un test verde daba confianza
sobre exactamente la línea rota.

La distinción que faltaba en los tres: **no lanzar** y **no adivinar** no son lo
mismo, y **no bloquear** y **no verificar** tampoco.

---

## Lo que NO se verificó

De los 46 hallazgos crudos verifiqué a mano 8. Los otros 38 (14 medios, 16 bajos,
más los altos de `paymentId` no atado al proof, timeouts del sink, y varios de
los SDKs) **siguen sin verificar** — la pasada adversarial nunca corrió. No los
descarto ni los confirmo. Quedan en `/tmp/.../wq0z47bjq.output`.

Dos que conviene mirar primero por lo que afirman:

- **`paymentId` no está atado a la transacción del proof** (`gate.rs:373`) — si
  es cierto, un autopago real satisfaría el gate de fase 2 para el `paymentId` de
  una víctima. Toca la mitad que hoy está apagada, pero es la que queremos
  encender.
- **Sin timeout en el POST del anchor ni en el sink** (`durable.rs:188`) — un
  facilitador colgado bloquearía la respuesta pagada. Roza la regla de §2.
