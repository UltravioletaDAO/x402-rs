# DX402 — Backlog: opt-in del comprador y monetización

**Fecha:** 2026-08-17
**Origen:** pregunta de Saul durante el despliegue de v1.76.0
**Estado:** investigación pendiente. **No implementado.**

---

## La pregunta

> ¿Se puede ofrecer la durabilidad como un tipo de servicio: el comprador la
> habilita al pagar, paga un poquito más, y le queda garantizado para siempre?
> ¿Es así como está hoy?

**No, hoy no es así.** Este documento dice qué hay realmente, qué falta, y cuál
parece el camino correcto.

---

## 1. Qué existe hoy (verificado en código)

| Pieza | Estado real |
|---|---|
| Opt-in **por ruta, decidido por el vendedor** | ✅ funciona. Una ruta declara `extensions: {"durable-evidence": {...}}` o no lo declara |
| `retention: "90d" \| "1y" \| "permanent"` | ✅ se respeta: lifecycle de S3 + TTL de DynamoDB |
| `paidBy: "seller" \| "buyer"` | ⚠️ **es solo una etiqueta**. `src/dx402/types.rs:165`. No hay sobreprecio, ni cobro aparte, ni liquidación del fee. No hace nada |
| Opt-in **del comprador** | ❌ no existe |
| Cobro diferencial | ❌ no existe |

Traducido: hoy el vendedor decide por todos sus compradores, y si alguien paga
por la durabilidad, es el vendedor, fuera de banda.

---

## 2. Lo importante: casi no hace falta protocolo nuevo

x402 ya devuelve un **array `accepts`** en el 402 — varias formas de pagar el
mismo recurso. El comprador elige cuál satisface.

Entonces el opt-in del comprador sale de ahí, sin inventar nada:

```jsonc
// 402 Payment Required
{
  "accepts": [
    {
      "scheme": "exact",
      "maxAmountRequired": "10000",        // 0.01 USDC — normal
      "resource": "https://kk.example/data/42",
      "extra": { }
    },
    {
      "scheme": "exact",
      "maxAmountRequired": "12000",        // 0.012 USDC — con evidencia durable
      "resource": "https://kk.example/data/42",
      "extensions": {
        "durable-evidence": {
          "mode": "direct",
          "retention": "permanent"
        }
      }
    }
  ]
}
```

El comprador (o su agente) elige la segunda si quiere que le quede. **El
vendedor ve cuál eligió** porque el `PaymentRequirements` que satisface viene en
el payload, y ahí decide si corre el post-hook.

Ventajas de esta forma:

- **Cero cambios al protocolo x402.** Cae directo del diseño multi-oferta que ya
  existe. Eso importa mucho para la propuesta upstream: una extensión que no pide
  cambiar el core se revisa mucho mejor.
- **El precio lo pone el vendedor**, no nosotros. No estamos en el medio de esa
  decisión comercial.
- **Un comprador viejo que no entiende DX402 elige la primera opción** y todo
  sigue funcionando. Degradación natural.

### Lo que sí habría que construir

1. **Que el post-hook lea qué requirement se satisfizo** y active la evidencia
   solo si ese trae `durable-evidence`. Hoy `with_durable_evidence(hook)` aplica
   a toda la ruta sin mirar el requirement elegido. Es el cambio central.
2. **Que `retention` salga del requirement elegido**, no de la config fija de la
   ruta.
3. Helpers en los SDKs para armar el par de ofertas sin escribirlo a mano.

---

## 3. La parte incómoda: el costo no justifica el precio

Saul dijo "eso ya está accounted for" respecto al storage. Vale la pena poner
números, porque cambian el argumento.

Un body típico de agente (1–50 KB) sellado pesa prácticamente lo mismo. En S3
Standard a ~$0.023/GB-mes:

| Escenario | Costo real |
|---|---|
| 50 KB, 90 días | ~**$0.0000035** |
| 50 KB, permanente, 10 años | ~**$0.00014** |
| 100.000 anclajes de 50 KB, permanentes, 10 años | ~**$14** |

*(Aritmética de la lista de precios, no medición. La real sale del primer mes con
tráfico.)*

**El storage es ruido.** Cobrar "para cubrir el storage" sería deshonesto: no hay
costo material que cubrir a esta escala.

Lo que sí tiene valor —y es lo que se estaría vendiendo— es otra cosa:

- **poder reclamar.** Sin evidencia no hay a quién reclamarle ni qué mostrar.
- **poder auditar** una compra meses después.
- **no repudio**: un recibo firmado que un tercero verifica sin llamarnos.

Eso es un producto legítimo. Pero el pitch honesto no es "cubrimos el storage",
es **"te queda prueba de qué te entregaron"**. Si el precio se justifica con el
costo, el primero que haga la cuenta nos deja mal parados.

> **Riesgo a mirar de frente:** si `permanent` se vende barato y alguien ancla
> millones de blobs, el costo sí aparece — y `permanent` es irrevocable, no se
> puede deshacer después. Cualquier oferta de `permanent` necesita un tope de
> tamaño y algún límite de volumen antes de existir.

---

## 4. Dónde cobra el facilitador (nosotros) — abierto

Distinguir dos cosas que se mezclan fácil:

- **El vendedor le cobra al comprador** por la durabilidad → sale del `accepts`,
  arriba. Ahí nosotros no tocamos plata.
- **Nosotros le cobramos al vendedor** por notarizar e indexar → eso todavía no
  tiene diseño ninguno.

Opciones a investigar para lo segundo, sin recomendación todavía:

1. Gratis para todo el mundo, siempre. Es infraestructura y el valor está en que
   se adopte el estándar. (Es lo que hacemos hoy con verify/settle.)
2. Gratis hasta N anclajes/mes por vendedor, después algo.
3. El vendedor paga el anclaje por x402 — cerrando el círculo: DX402 se cobra a
   sí mismo con el protocolo que extiende. Elegante, y hay que ver si no es
   demasiado cute.

**Sin decidir.** Y no se decide antes de tener tráfico real: hoy no sabemos si el
volumen es de decenas o de millones al mes, y la respuesta cambia con eso.

---

## 5. Orden sugerido

1. **Primero probar que funciona** (KarmaCadabra, el objetivo de hoy). Sin un
   solo anclaje real, todo esto es especulación.
2. Después el opt-in del comprador vía `accepts` — es el cambio que más habilita
   y el que menos protocolo toca.
3. La monetización del facilitador al final, con números reales en la mano.

---

## 6. Para la propuesta upstream

Que el opt-in salga del `accepts` que x402 **ya tiene** es un argumento fuerte
frente a la Foundation: la extensión no pide tocar el core, se apoya en un
mecanismo existente, y degrada sola con clientes viejos.

Vale la pena que el spec v0.2 lo diga explícitamente, con el ejemplo del array de
dos ofertas.
