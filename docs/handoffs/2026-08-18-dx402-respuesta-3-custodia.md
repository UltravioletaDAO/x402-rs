# Una de sus conclusiones es corregible: la custodia no les impide ser destinatarios

**Para:** equipo de KarmaKadabra
**De:** facilitador (x402-rs), 2026-08-18
**Responde a:** `karmakadabra/docs/handoffs/2026-08-18-dx402-firmamos-y-abrimos-nuestra-evidencia.md`

---

## 0. Lo importante primero

**Su vendedor en custodia SÍ puede ser destinatario del sobre.** La clave de
**cifrado** no tiene por qué ser la de **cobro** — y no debería serlo.

Lo probé punta a punta:

```
roles                       : ['payer', 'seller']
abre el COMPRADOR           : True
abre el VENDEDOR (custodia) : True
una wallet ajena            : rechazada
```

El vendedor genera un keypair **local** que se usa **solo para descifrar
evidencia**: sin fondos, sin gas, sin custodio de por medio. Lista esa clave
pública como destinatario `seller` y listo. La wallet de cobro sigue en Paybox y
no participa.

Y hay una razón para separarlas más allá de esto: usar la clave con la que cobrás
para también descifrar mezcla roles. Una filtración pasaría de *"leen mi
evidencia"* a *"me vacían la wallet"*.

Ya está en `docs/DX402.md`.

**La otra mitad de su hallazgo sí es un hueco real**: firmar el anchor necesita
la clave del payee, porque esa firma **es** el reclamo de que controlás `payTo`.
Eso no se delega a otra clave sin diseño. Detalle en §2.

---

## 1. Actualicen a 0.52.1

Shippearon contra **0.52.0** y hay un arreglo encima que les va a importar si
alguien monta el entorno desde cero:

Hasta 0.52.0 el extra `dx402` **no declaraba un backend de hashing**. Una
instalación limpia revienta en la primera llamada a keccak:

```
ImportError: None of these hashing backends are installed: ['pycryptodome', 'pysha3']
```

Eso es todo `content_hash`, `payment_id` y `anchor_digest` — el extra entero
inútil en un entorno nuevo. A ustedes no les pasó porque ya tenían `pycryptodome`
por otra dependencia; a nosotros tampoco, porque los tests corren en un entorno
que ya lo tenía. Lo encontró instalar el paquete publicado en un venv vacío.

**`pip install -U 'uvd-x402-sdk[dx402]>=0.52.1'`**

Es el mismo error que discutimos en su §4, otra vez: un chequeo verde que no
cubría lo que estaba roto.

---

## 2. La custodia, separada en tres

Vale distinguirlas porque sólo dos son huecos:

| Qué necesita | ¿Bloquea a un custodio? |
|---|---|
| **Ser destinatario del sobre** (ECDH) | **No** — clave de cifrado aparte, arriba |
| **Firmar el anchor** | **Sí**, si el custodio no firma bytes arbitrarios |
| **Descifrar como comprador** | **Sí** — es el caso original de `escrowed` |

Sobre la segunda: **un custodio que sí firme digests crudos no necesita nada**.
Su decisión de separar digest y firma en dos pasos es exactamente lo que hace
falta, y es la razón por la que funciona donde funciona.

Donde el custodio sólo firma transacciones y no mensajes, hoy no hay salida y sus
anchors quedan provisionales. **No es urgente** —nadie se los puede quitar sin
firmar tampoco— pero tampoco pueden reclamarlos. Quedan dos caminos en el
backlog, ninguno decidido:

1. **Delegación de autoridad de anclaje**: declarar una clave de firma de anchors
   distinta de `payTo`. El problema es cómo sabemos que esa clave está autorizada
   — probarlo pide una firma de `payTo`, que es justo lo que falta. Quizá una sola
   vez, al registrarse.
2. **Modo `escrowed`**, que resuelve el lado comprador y de paso este.

Es la tercera vez que aparece el mismo caso de uso con distinta ropa. Queda
anotado con su nombre.

---

## 3. Sobre cómo verificaron la firma

> El digest no se puede verificar leyendo — o coincide byte por byte con el suyo o
> la firma no vale nada, y una comparación local sólo prueba que uno es
> consistente consigo mismo.

Eso es exactamente el criterio correcto, y es más riguroso que el que aplicamos
nosotros al principio de todo esto. Usar la semántica del supersede como oráculo
—anclar sin firma, después con firma, y comprobar que la segunda gana— es un test
de aceptación mejor que comparar bytes, porque prueba el efecto y no la forma.

Y borrar su implementación después de confirmar que coincidía es la decisión
correcta por la razón que dan: acertar una vez no es un invariante que alguien
pueda mantener.

Su documentación del `chainId: 0` va a la guía tal cual, con crédito. Tienen razón
en que nadie lo adivina: sale de que `chain_id_of` parsea el CAIP-2 y sólo
`eip155:` devuelve número. Y un `chainId` equivocado **no produce ningún error en
ningún lado** — sólo una firma que no verifica contra nada.

---

## 4. Estado

| Qué | De quién | Estado |
|---|---|---|
| Sobre bidireccional con wallet en custodia | ustedes | ✅ **se puede hoy** — clave de cifrado aparte |
| `chainId: 0` y el detalle de custodia en la guía | nosotros | ✅ |
| Backend de hashing del extra | nosotros | ✅ **0.52.1**, actualicen |
| Firmar anchors desde un custodio que no firma bytes | ambos | backlog, dos caminos |
| Modo `escrowed` | nosotros | backlog, tercer pedido del mismo caso |
| Gate on-chain de Solana | nosotros | backlog — ya no bloquea el secuestro |
| DX402 en el decorador de los 5 sellers | ustedes | bloqueado por lo suyo |

Nada pendiente de nuestro lado que los bloquee.
