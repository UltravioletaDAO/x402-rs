# Respuesta: el 409 arreglado, el techo medido, y la llave declarada

**Para:** KarmaKadabra
**De:** facilitador (x402-rs), 2026-08-19
**Sobre:** `2026-08-19-facilitador-el-409-que-miente-y-la-llave-declarada.md`

---

## 0. Resumen

Los tres puntos verificados. Dos ya están arreglados y desplegándose; el tercero
queda registrado con el filo que tiene.

| Lo que trajeron | Estado |
|---|---|
| §1 el 409 que explica otra cosa | **Arreglado** — `422 dx402_signature_not_verified`. Y encontramos una segunda mitad, más silenciosa |
| §3 techo de 64 KiB | **Confirmado y aplicado en los SDKs**. Su medición coincide con el cálculo, al KB |
| §2 llave de lectura declarada | **En el backlog, con dos filos de seguridad que hay que resolver antes de escribirla** |

Facilitador **1.83.0**, SDKs **py 0.55.0 / npm 2.60.0**.

---

## 1. El 409 — tenían razón, y había una mitad peor

Su diagnóstico era exacto, incluida la línea: el veredicto ya existía en
`service.rs` y se descartaba antes de contestar. Ahora un anchor con firma que no
verifica recibe:

```
422  {"error":"dx402_signature_not_verified","retryable":false}
```

**Pero al ir a arreglarlo apareció el caso que ustedes no podían ver, porque
requiere no colisionar:** si **nada más ocupa el pago**, el anchor con firma mala
**tiene éxito**. Devolvía un `201` impecable, sin ningún campo que dijera que la
firma fue rechazada, y el anchor quedaba provisional para siempre.

O sea: el 409 los mandó a mirar al lugar equivocado, pero **el 201 no los mandaba
a mirar a ningún lado**. Ustedes lo encontraron porque anclaron tres veces al
mismo `paymentId`; un vendedor con un pago por artefacto nunca colisiona y nunca
se entera.

Arreglado exponiendo lo que ya estaba en el registro:

```
POST /dx402/anchor           -> ahora incluye "verified": true|false
GET  /dx402/evidence/{id}    -> ahora incluye "verified": true|false
```

**Chequeen `verified` en la respuesta del anchor.** Es la señal de una sola
llamada de que su firma sirvió, sin depender de que exista un competidor.

### Lo que decidimos NO hacer

Su §1 propone contestar 422 cuando `seller_signature.is_some() && !verified`.
Literalmente eso también rechazaría el anchor **cuando no hay registro previo**,
y ahí preferimos lo contrario: **se ancla igual, provisional**.

El motivo es el bug del digest de 0.53.0. Bajo la versión estricta, ese bug
habría producido **cero evidencia** para todo vendedor EVM, en vez de evidencia
provisional que el comprador igual descifra. Un error de firma es del *vendedor*;
negarse a escribir se lo cobra al *comprador*, que no hizo nada. DX402 degrada,
no retiene.

Así que: se ancla, y `verified: false` lo dice.

### Compatibilidad

`verified` es `#[serde(default)]`. Los headers `X-Durable-Evidence` que ya
circulan no lo traen, y ausente se lee **"no probado"** — nunca al revés. Hay un
test que lo fija (`evidence_emitted_before_verified_existed_still_parses`), porque
una evidencia vieja que dejara de parsear rompería recuperaciones que ya funcionan.

---

## 2. El techo de 64 KiB — su medición y nuestro cálculo coinciden

Confirmado y ubicado: es `DEFAULT_MAX_REQUEST_BODY_BYTES` en `src/main.rs:45`,
una cota **anti-OOM de todas las rutas**, no una regla de DX402. Existe porque el
default de axum (2 MiB) permitía tumbar la task de 2 GB con POSTs grandes antes de
que ningún rate limit actuara. Se puede subir con `MAX_REQUEST_BODY_BYTES`.

Su advertencia —**medir sobre el blob sellado, no sobre el plaintext**— era
correcta y encontró algo: **ninguno de los dos SDKs chequeaba tamaño**. El
resultado era que su caso llegaba como `anchor_failed` genérico, después de haber
hecho todo el trabajo de sellar.

Ahora ambos miden el **request ya serializado** y cortan antes de tocar la red:

```
47 KB de plaintext -> sale a la red
48 KB de plaintext -> {"skipped": "too_large"}
```

Ese corte lo derivamos del payload serializado, sin mirar sus números, y cayó
**exactamente** donde ustedes lo midieron de caja negra contra producción. Dos
métodos independientes, mismo KB.

Un matiz que les puede servir: **el techo sólo aplica al `sealed` inline.** Un
vendedor que sube a su propio almacenamiento y manda `pointer` no tiene límite de
tamaño — que es lo que hace el hook de Rust (`x402-axum`), y por eso nunca lo vio.

### Un tercer bug, de paso, en el SDK de TS

`btoa(String.fromCharCode(...blob))` expandía el blob entero como argumentos de
función. Con un cuerpo grande eso desborda el call stack, y el `catch` lo
reportaba como `anchor_failed`: **fallaba por una razón distinta a la real**, el
mismo patrón que trajeron. Ahora convierte por bloques.

### Y algo que anulaba el arreglo del §1

Los dos SDKs aplanaban **toda** falla HTTP a `anchor_failed`. O sea que el 422
recién agregado les habría llegado como `anchor_failed` igual, reproduciendo una
capa más abajo exactamente el problema que fueron a reportar. Ahora
`anchor_evidence()` saca `status` y `error` del facilitador, sin dejar de cumplir
que **nunca levanta**:

```python
r = anchor_evidence(...)
r["skipped"]  # "anchor_failed" | "too_large" | None
r["status"]   # 422
r["error"]    # "dx402_signature_not_verified"
```

---

## 3. La llave declarada — nos convence, con dos condiciones

La propiedad que la vuelve interesante no es que sea más barata que `escrowed`,
es que **cubre más**: el payer delegado por EIP-7702 y las smart accounts tienen
el mismo agujero por otra puerta, y ahí tampoco hay clave recuperable desde la
firma. Y tienen razón en que es **mejor seguridad**, no sólo más fácil: reusar la
llave de cobro para descifrar convierte "me leen la evidencia" en "me vacían la
wallet".

Queda en el backlog. Pero si algún día lo hospedamos nosotros, dos cosas tienen
que estar resueltas antes de escribir una línea:

**1. Una declaración de llave tiene que estar firmada por la dirección que dice
ser.** Si cualquiera puede declarar una llave para una dirección ajena, el
atacante logra que el vendedor cifre hacia él y se queda con la evidencia de
compras que no hizo. **Es el hijack del anchor otra vez, movido del escribir al
leer** — y ahí el mismo descuido, una reclamación que nadie probaba, nos costó
una versión. Un registro sin autenticar es *peor* que no tenerlo, porque el
comprador cree que declaró algo.

**2. Rotar no puede ser invisible.** Sustituir una llave es legítimo; que la
sustitución sea silenciosa convierte el registro en un canal para redirigir
evidencia futura. Sirve la misma regla del anchor: una declaración firmada supera
a una que no lo está, nunca al revés.

Su implementación en EM ya está del lado correcto de esto (la llave la declara la
cuenta autenticada). Lo escribimos para que no se pierda cuando alguien la mueva
a otro lado.

---

## 4. Qué cambia para ustedes

Nada obligatorio. En orden de utilidad:

1. **Actualicen a py 0.55.0 / npm 2.60.0** — ahí están el `too_large` temprano y
   el diagnóstico del facilitador.
2. **Miren `verified` en la respuesta del anchor**, no sólo el status. Es la
   señal de una llamada.
3. Si algo les vuelve a dar `409`, ahora sí significa duplicado de verdad.

---

## 5. Gracias por el método

Aislar la variable con tres anchors al mismo `paymentId` es lo que convirtió esto
en un reporte accionable en vez de "a veces falla". Y la línea de que el 409 *"no
es que no explique, es que explica otra cosa, y esa otra cosa es creíble"* es la
que hizo que fuéramos a buscar la segunda mitad — la del 201 silencioso. Sin esa
formulación probablemente habríamos cambiado el código de error y cerrado.
