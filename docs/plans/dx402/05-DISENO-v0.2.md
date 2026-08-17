# DX402 v0.2 — Cerrar el gate del anchor y el opt-in del comprador

**Fecha:** 2026-08-17
**Origen:** dos preguntas de Saul después de que v1.77.0 quedó vivo
**Estado:** diseño. No implementado. Cierra los dos bloqueantes de upstream.

---

## Parte 1 — Cerrar `POST /dx402/anchor`

### El hueco, dicho sin adornos

Hoy el endpoint acepta cualquier `paymentId` / `txHash`. No consulta nada.
Cualquiera puede anclar ~48 KB sin pagar, y obtener un recibo **firmado por
nosotros** para un pago que nunca existió.

### Sí: se usa el `ProofOfPayment` que ya existe

No hay que inventar nada. `ProofOfPayment` **ya se produce en el settle**
(`create_proof_of_payment`, `src/chain/evm.rs:1317`) y **ya viaja en la respuesta
del settle**. El vendedor lo recibe y lo devuelve en el anchor. El facilitador lo
re-verifica con el mismo módulo que ya escribimos para ERC-8004
(`src/erc8004/proof.rs`).

Los 8 pasos de `verify_proof_of_payment` mapean casi uno a uno:

| # | Qué chequea en ERC-8004 | Equivalente en DX402 |
|---|---|---|
| 1 | misma cadena que el feedback | misma cadena que el anchor |
| 2 | `payment_hash` recomputa | **idéntico** — si no, el struct fue editado después del settle |
| 3 | `payer == rater` | **`payer` == la address a la que se está cifrando** ← el que más importa |
| 4 | la tx existe y tuvo éxito | idéntico |
| 5 | está en el bloque que declara | idéntico |
| 6 | frescura contra el timestamp del bloque | idéntico, pero **ventana corta** (ver abajo) |
| 7 | el `Transfer` está en esa tx | idéntico |
| 8 | el payee es ese agente | **el payee es el vendedor que ancla** ← necesita otra atadura |

### El paso 3 es el que cierra el agujero de verdad

No es solo "hubo un pago". Es que **la clave a la que se cifró la evidencia
pertenece a quien efectivamente pagó**. Sin eso, alguien podría anclar evidencia
cifrada hacia *su propia* clave y colgarla del pago de otro.

### El paso 8 necesita algo que ERC-8004 resuelve con un registry

Allá, el Identity Registry ata `payee → agentId`. Acá no hay registry. Dos
opciones:

**A. El vendedor firma el anchor** (recomendada). Una firma EIP-712 sobre
`(paymentId, contentHash, pointer)` con la clave de `payTo`. Prueba que quien
ancla es quien cobró. ~30 líneas, sin infraestructura nueva.

**B. Solo comparar `payee` contra el declarado.** Más barato, pero deja una
carrera: alguien que observa la tx podría anclar basura —cifrada correctamente
hacia el pagador— antes que el vendedor, y el anti-replay por `paymentId` le
cierra la puerta al vendedor legítimo.

**Va la A.** La B convierte el anti-replay en un arma.

### Ventana de frescura: minutos, no días

ERC-8004 usa 7 días (`ERC8004_PROOF_MAX_AGE_SECS = 604800`) porque alguien puede
opinar sobre una compra una semana después. DX402 no: el anchor ocurre **dentro
del mismo handler** que el settle. Una ventana de ~15 minutos alcanza y reduce
mucho la superficie.

### Anti-replay

`paymentId` es único por pago. Un pago ancla **una vez**. Hoy un segundo anchor
con el mismo id pisa el registro — hay que rechazarlo. La tabla ya tiene
`payment_id` como hash key, así que es un `ConditionExpression:
attribute_not_exists(payment_id)` en el `PutItem`.

### Rollout: fase 1 primero, igual que ERC-8004

`DX402_REQUIRE_PROOF=false` → **verifica y reporta, no rechaza**. Se miran los
logs hasta ver que el tráfico real pasa, y recién ahí se cierra. Prender un gate
sin haber visto tráfico es cómo se rompe a los integradores que ya andaban.

### La limitación honesta: esto es EVM-only

`verify_proof_of_payment` devuelve `NotEvmTransaction` para todo lo que no sea
EVM. En Solana no hay un receipt EVM que leer — es el mismo verdicto
`proof_unverifiable_chain` que ya tiene ERC-8004.

**Y eso choca con lo que le dijimos a KarmaCadabra** ("empiecen por Solana",
porque ahí la address ES la clave). Sigue siendo el mejor consejo para el
cifrado, pero el gate llega después a esa cadena.

Para Solana hay que escribir una verificación propia: `getTransaction` + parseo
de los token balances, que es más o menos lo que ya hace
`src/chain/solana.rs` en el camino del settlement account. Es código nuevo, no
reuso.

**Orden sugerido:** cerrar EVM primero (reuso puro), Solana después. Y mientras
tanto **no** dejar Solana sin gate: si no se puede verificar, `direct` sigue
siendo seguro para el comprador (el cifrado no depende del gate), lo que queda
abierto es el abuso de storage. Un rate limit por `payee` lo contiene hasta que
llegue la verificación real.

---

## Parte 2 — El flujo del comprador que quiere durabilidad

### Lo primero: el facilitador NO puede cifrar

Vale repetirlo porque es la restricción que ordena todo el diseño: **el
facilitador nunca ve el body.** Solo participa en `/verify` y `/settle`. El
cifrado tiene que hacerlo el vendedor, porque es el único que tiene el
plaintext.

Lo que sí hace el facilitador, y es casi todo lo demás:

- verifica que el pago existe (Parte 1)
- guarda el ciphertext
- firma el recibo
- indexa por `paymentId`
- sirve el blob para siempre después

Del lado del vendedor queda **una línea** con el middleware. "El facilitador se
encarga" es cierto en todo salvo el acto de cifrar, y eso no se puede delegar sin
romper la propiedad que hace valioso a DX402.

### El flujo, paso a paso

```
┌── 1. El comprador pide el recurso ────────────────────────────────────┐
│  GET /data/42                                                          │
└────────────────────────────────────────────────────────────────────────┘
                              ▼
┌── 2. El vendedor responde 402 con DOS opciones ───────────────────────┐
│  {                                                                     │
│    "accepts": [                                                        │
│      { "maxAmountRequired": "10000",     // 0.01 USDC                  │
│        "resource": "https://kk.xyz/data/42" },                         │
│                                                                        │
│      { "maxAmountRequired": "12000",     // 0.012 USDC                 │
│        "resource": "https://kk.xyz/data/42",                           │
│        "extensions": {                                                 │
│          "durable-evidence": {                                         │
│            "retention": "permanent",                                   │
│            "recipients": ["payer", "seller"]                           │
│          } } }                                                         │
│    ] }                                                                 │
└────────────────────────────────────────────────────────────────────────┘
                              ▼
┌── 3. El comprador ELIGE ──────────────────────────────────────────────┐
│  Firma una autorización EIP-3009 por 0.012 (la opción con evidencia).  │
│  UN solo pago. No hay cobro aparte ni segundo round-trip.              │
└────────────────────────────────────────────────────────────────────────┘
                              ▼
┌── 4. GET /data/42  con  X-PAYMENT ────────────────────────────────────┐
                              ▼
┌── 5. El vendedor liquida ─────────────────────────────────────────────┐
│  POST /settle  →  facilitador  →  cadena                               │
│  ← SettleResponse { txHash, payer, proofOfPayment }                     │
└────────────────────────────────────────────────────────────────────────┘
                              ▼
┌── 6. El vendedor ve CUÁL opción se satisfizo ─────────────────────────┐
│  El PaymentRequirements que el comprador cumplió viene en el payload.  │
│  ¿Trae `durable-evidence`? → corre el post-hook. ¿No? → no hace nada.  │
└────────────────────────────────────────────────────────────────────────┘
                              ▼
┌── 7. Sella ───────────────────────────────────────────────────────────┐
│  clave del comprador ← recuperada de la firma del pago (gratis)        │
│  clave del vendedor  ← declarada en su config                          │
│  CEK → AES-256-GCM(body); CEK envuelta hacia AMBOS                     │
└────────────────────────────────────────────────────────────────────────┘
                              ▼
┌── 8. Ancla ───────────────────────────────────────────────────────────┐
│  POST /dx402/anchor { sealed, contentHash, proofOfPayment, sellerSig } │
│  El facilitador VERIFICA el pago, guarda, firma el recibo, indexa.     │
└────────────────────────────────────────────────────────────────────────┘
                              ▼
┌── 9. El comprador recibe ─────────────────────────────────────────────┐
│  200 OK + el body + X-Payment-Response + X-Durable-Evidence            │
└────────────────────────────────────────────────────────────────────────┘
                              ▼
        ...meses después, con la misma wallet con la que pagó...
┌── 10. Recupera ───────────────────────────────────────────────────────┐
│  GET /dx402/blob/{paymentId} → ciphertext → descifra local             │
│  Nadie le pide permiso a nadie: es aritmética.                          │
└────────────────────────────────────────────────────────────────────────┘
```

### Respuestas directas a lo que preguntaste

**"¿Me va a cobrar un poquito más antes de darme el proof of payment?"**
No hay un cobro extra ni un momento aparte. Es **el mismo pago, por un monto
mayor**. El comprador firma una sola autorización, por 0.012 en vez de 0.01. El
`ProofOfPayment` sale de ese mismo settle.

**"¿Qué opciones hay para pagar en ese momento?"**
Las que el vendedor ponga en `accepts`. Puede ofrecer dos (con y sin evidencia),
tres (90d / 1 año / permanente a precios distintos), o una sola con evidencia
incluida si quiere que sea obligatorio. **El precio lo pone el vendedor**, no
nosotros.

**"¿Y si el comprador no entiende DX402?"**
Elige la primera opción y todo funciona como hoy. Degradación natural, sin
código condicional en ningún lado.

**¿Quién se queda los 0.002 extra?**
El vendedor. Él fijó el precio y él paga el anclaje. Si en algún momento el
facilitador quiere cobrar por notarizar, es un diseño aparte — hoy no cobra nada.

### Por qué esta forma y no otra

Alternativas que consideré y descarté:

| Alternativa | Por qué no |
|---|---|
| Un **segundo pago x402** solo por la evidencia | Round-trip extra, y para cuando ocurre el body ya se entregó. Llega tarde por construcción. |
| El facilitador le **cobra al vendedor** por mes | No es opt-in del comprador. El comprador no puede pedir durabilidad. |
| Usar el scheme **`upto`** (monto variable) | Funcionaría, pero es maquinaria pesada para un delta de precio fijo. |
| Un **header** `X-Want-Evidence` | No está firmado. Cualquiera lo pone y el vendedor no cobró por él. |

La del `accepts` gana por algo que importa mucho de cara a la Foundation:
**no toca el core de x402**. El array multi-oferta ya existe y ya se usa para
ofrecer distintas cadenas o tokens. Ofrecer distintas *garantías* es el mismo
mecanismo.

---

## Parte 3 — Bidireccionalidad: de dónde sale la clave del vendedor

El envelope multi-destinatario está descrito en
`04-BACKLOG-MONETIZACION.md` §2-bis. Lo que faltaba resolver era la clave del
vendedor, y es el detalle que más fácil se hace mal:

**`payTo` no sirve.** En EVM una address es un hash: no da la clave pública. Y
aunque se pudiera recuperar de alguna tx del vendedor, usar la clave con la que
cobra para también descifrar es mezclar roles — una filtración pasaría de "leen
mi evidencia" a "me vacían la wallet".

**El vendedor declara una clave de cifrado aparte**, en la config de la ruta:

```jsonc
"durable-evidence": {
  "recipients": ["payer", "seller"],
  "sellerPublicKey": "0x02a1b2…"   // secp256k1 comprimida, o X25519 en ed25519
}
```

Sin fondos, sin gas, rotable sin tocar nada más. Igual que la clave con la que el
facilitador firma recibos.

**Y va en el recibo.** Los destinatarios son parte de la atestación firmada, no
metadata suelta. Un comprador tiene que poder ver que el vendedor también puede
leer lo que compró — descubrirlo después sería exactamente la sorpresa que
arruina la propiedad de privacidad.

---

## Orden de implementación

1. **Gate del anchor en EVM** (Parte 1) — reuso puro de `proof.rs`, fase 1
   primero. Es el que cierra un abuso que está abierto **hoy en producción**.
2. **Envelope multi-destinatario** (Parte 3) — cambia el formato en disco, así
   que cuanto antes mejor: mientras menos evidencia haya anclada en v1, más
   barata la migración.
3. **Opt-in vía `accepts`** (Parte 2) — no toca el formato, solo la declaración
   y el post-hook. Puede ir después sin costo.
4. **Gate del anchor en Solana** — código nuevo, no reuso.

Recién con 1 + 2 + 3 tiene sentido escribir el PR a la x402 Foundation.
