# De Sepolia a Ethereum mainnet — gap técnico del scheme `fhe-transfer`

**Fecha:** 2026-08-31 (actualizado el mismo día: 5c resuelto, hallazgo 6 agregado)
**Estado:** análisis. Solo el punto 5c (unificar timeouts) está hecho; el resto no.
**Relación:** es el Milestone 1 de `01-APPLICATION.md`

> **QUÉ:** el inventario de lo que falta para que `fhe-transfer` liquide en
> Ethereum mainnet.
> **POR QUÉ:** hoy la ruta existe y funciona, pero el Lambda solo declara
> `fhevm-local` y `sepolia`. Mainnet no es una variable de entorno: es config
> nueva, un token nuevo, una wallet nueva y un secreto nuevo.
> **RIESGO:** si se anuncia `fhe-transfer` en mainnet sin cerrar los puntos 1 a 4,
> el facilitador ofrece un scheme que revienta en `/settle` — y el que se lleva el
> golpe es un vendedor con un pago real encima.

---

## Punto de partida (verificado 2026-08-31)

Funciona end-to-end en Sepolia. Se comprobó que un `POST /verify` con
`scheme: fhe-transfer` contra `facilitator.ultravioletadao.xyz` llega hasta el
Lambda y devuelve su validación. Los tres saltos están vivos.

```
ECS (x402-rs v2.0.0)  ->  src/fhe_proxy.rs  ->  zama-facilitator...xyz  ->  Zama @ Sepolia
```

`GET /health` del Lambda: `{"networks":["fhevm-local","sepolia"]}` — ahí está el techo.

---

## Los huecos

### 1. El Lambda no conoce mainnet

**Estado:** bloqueante.

`zama-facilitator` declara `fhevm-local` y `sepolia`. Para mainnet hace falta la
configuración del protocolo en Ethereum: `GATEWAY_URL`, `ACL_CONTRACT_ADDRESS`,
`KMS_CONTRACT_ADDRESS` y el relayer, todos con los valores de mainnet —
distintos de los de Sepolia.

Las direcciones se sacan de la documentación del Zama Protocol, **no de memoria
ni de un ejemplo viejo**. Una dirección de ACL equivocada no falla ruidosamente:
falla como "no autorizado a descifrar", que se lee igual que un problema de
permisos del usuario.

**Verify:** `GET /health` del Lambda lista `ethereum` (o `mainnet`) entre sus
`networks`, y un `/verify` contra un handle cifrado de mainnet resuelve.

---

### 2. `/supported` no anuncia la red

**Estado:** trivial, pero va después del punto 1.

`src/facilitator_local.rs:219-233` registra hoy dos entradas —
`ethereum-sepolia` (v1) y `eip155:11155111` (v2). Faltan las de mainnet:
`ethereum` (v1) y `eip155:1` (v2), con `extra: None` igual que las actuales,
porque el fee payer lo resuelve el Lambda.

**Orden que importa:** el Lambda primero, el anuncio después. Anunciar una
capacidad que el backend no tiene es exactamente cómo un cliente descubre un
scheme y se estrella al liquidar.

**Verify:**
```bash
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq '[.kinds[] | select(.scheme=="fhe-transfer")]'
```
Debe devolver cuatro entradas, no dos.

---

### 3. No hay token confidencial de mainnet configurado

**Estado:** bloqueante, y es trabajo de investigación además de código.

Hace falta un ERC7984 real en Ethereum mainnet como activo cobrable — cUSDT o
cUSDC. Hay que resolver:

- La dirección del contrato, **verificada contra la fuente de Zama/OpenZeppelin**, no contra un post de blog
- Decimales y cómo se representa el monto cifrado en el payload
- Si entra o no en `scripts/stablecoin_matrix.py` (probablemente no: ese script parsea `src/network.rs`, y el activo FHE no vive ahí porque el settlement lo hace el Lambda). **Decidirlo explícitamente**, no dejarlo ambiguo.

**Riesgo directo de pérdida de fondos:** una dirección de token equivocada en un
path de settlement manda dinero a un contrato sin función de retiro. Esto se
verifica dos veces contra dos fuentes independientes antes de tocar mainnet.

**Verify:** `eth_getCode` contra la dirección en mainnet usando dos RPC
independientes, y una transferencia confidencial de monto mínimo liquidada de
punta a punta.

---

### 4. No hay wallet de settlement en mainnet

**Estado:** bloqueante, y tiene costo.

El Lambda firma el settlement. En Sepolia eso es gas gratis; en mainnet es ETH
real, y las operaciones FHE son caras comparadas con una transferencia ERC-20.

- Wallet nueva y separada para mainnet FHE — **no reusar** la de testnet ni la EVM mainnet del facilitador (`0x1030...13C7`). Una clave por dominio de riesgo.
- La clave va a **AWS Secrets Manager**, con la convención del repo (`facilitator-<chain>-<env>-<kind>`).
- Fondearla con ETH y medir el costo de gas de un settlement FHE antes de anunciar la red. Si un pago de $0,01 cuesta $3 de gas, el scheme no es usable en L1 y eso hay que saberlo **antes**, no después.

**Nota de política del repo:** la clave nunca se escribe en un archivo, ni como
ejemplo, ni "temporalmente". Se lee de Secrets Manager en runtime.

**Verify:** balance de la wallet consultado on-chain + un settlement de prueba
con su costo de gas medido y anotado.

---

### 5. El terraform no cubre mainnet, y la URL del Lambda vive en el código

**Estado:** no bloqueante para que funcione, pero deja deriva invisible.

Tres cosas separadas:

**a) `FHE_FACILITATOR_URL` no está en el terraform de producción.** El
contenedor corre con el default hardcodeado en `src/fhe_proxy.rs:31`
(`https://zama-facilitator.ultravioletadao.xyz`). Funciona hoy, pero significa
que apuntar el facilitador a otro backend FHE exige recompilar. Debe declararse
en `terraform/environments/production/main.tf` como cualquier otra config del
servicio.

**b) El terraform del Lambda es un environment `zama-testnet` con `sepolia`
cableado.** `variables.tf` fija `environment = "testnet"` y el dominio
`zama-facilitator.ultravioletadao.xyz`; la IAM policy solo concede
`GetSecretValue` sobre el ARN del secreto `sepolia_rpc`
(`main.tf:135`). Mainnet necesita su propio secreto de RPC y su propio grant —
no alcanza con cambiar una variable.

**c) Inconsistencia de timeout — RESUELTA (2026-08-31).** Había tres valores
para la misma cosa: 30s en el terraform, 60s en un comentario de
`src/fhe_proxy.rs`, y 90s en el cliente que ese archivo construye de verdad. El
proxy esperaba 90s por un Lambda que AWS mataba a los 30.

Ahora hay una sola definición, `fhe_request_timeout_secs = 90` en
`terraform/environments/zama-testnet/variables.tf`, y todo deriva de ahí: el
timeout del Lambda, el de la integración de API Gateway, la alarma de duración
(80% de ese valor), y — vía `FHE_PROXY_TIMEOUT_SECS`, que pasa el terraform de
producción — el cliente Rust. El default en el código coincide, así que un
entorno sin la variable se comporta igual. Un valor basura o fuera de rango
loguea warning y cae al default; no tumba el arranque.

**Verify:** `terraform output fhe_request_timeout` en el workspace `zama-testnet`
imprime el valor efectivo en cada salto.

---

### 6. El techo de 30s de API Gateway (hallazgo nuevo)

**Estado:** bloqueante para operaciones FHE lentas en mainnet. **No lo teníamos
identificado.**

Salió al unificar los timeouts. El Lambda está detrás de un **API Gateway HTTP
API**, y ahí el timeout de integración tope es **30 segundos**, con AWS marcando
esa cuota como **"Can be increased: No"**
([HTTP API quotas](https://docs.aws.amazon.com/apigateway/latest/developerguide/http-api-quotas.html)).
No es un default que se sube pidiendo un aumento de cuota: el aumento más allá
de 29s existe solo para REST APIs regionales y privadas, no para HTTP APIs.

O sea: **el stack está de acuerdo en 90s y el cliente igual se corta a los 30.**
El terraform ahora clampea la integración a 30000ms explícitamente para que el
techo se vea en el código y no solo en un 504, pero el clamp no lo levanta.

Peor que un timeout corto: pasados los 30s el llamador recibe 504 y **el Lambda
sigue corriendo y facturando** hasta su propio timeout, sin nadie del otro lado.

**Por qué importa justo para mainnet:** en Sepolia con montos de prueba nadie se
acerca a 30s. Las operaciones FHE de mainnet son más pesadas y el relayer tiene
más cola. Este techo es exactamente el tipo de cosa que no se manifiesta hasta
que hay tráfico real.

**Salida:** poner un **Lambda Function URL** delante en vez del HTTP API — hereda
el timeout de la función (hasta 15 min). Cambia el punto de entrada, así que
arrastra el dominio custom, CORS y los access logs. **Decisión de arquitectura,
no se toca sin acuerdo.**

> Guardado con sus cuatro opciones, el modo de falla concreto y el disparador
> que lo vuelve bloqueante en **`docs/plans/backlog-fhe-gateway-timeout.md`**.

**Verify:** medir cuánto tarda de verdad un `/verify` y un `/settle` FHE contra
mainnet **antes** de anunciar la red. Si el p99 pasa de ~25s, este punto se
vuelve bloqueante duro.

---

## Orden de ejecución

```
5c. Unificar timeouts                   HECHO (2026-08-31)
1.  Config de mainnet en el Lambda      (bloqueante, primero)
3.  Token ERC7984 de mainnet            (bloqueante, en paralelo con 1)
4.  Wallet + secreto + fondeo           (bloqueante, después de 3)
    -> medir gas de un settlement real  <- GO/NO-GO del scheme en L1
    -> medir LATENCIA de verify/settle  <- decide si el punto 6 bloquea
6.  Function URL en vez de HTTP API     (solo si la latencia se acerca a 30s)
5ab. FHE_FACILITATOR_URL + tf de mainnet (limpieza, antes de anunciar)
2.  Anunciar en /supported              (ÚLTIMO, cuando lo de arriba funciona)
```

El punto 2 va al final a propósito. Es el único cambio que un tercero puede ver,
y verlo significa "esto ya sirve".

---

## Lo que NO entra en este alcance

- Otras chains con FHEVM. Primero Ethereum mainnet, con tráfico real.
- Optimizar el cold start del Lambda más allá de la concurrencia aprovisionada que ya existe (`provisioned_concurrency_count = 1`).
- Circuit breaker y métricas FHE dedicadas (están en "Future Enhancements" de `docs/ZAMA_FHE_INTEGRATION.md`). Son buenas ideas y no son requisito para el primer pago en mainnet.
- Proponer el scheme upstream a la x402 Foundation — eso es el Milestone 3, y exige tráfico real primero.

---

## Regla que no se negocia

**Nada de esto se despliega por iniciativa propia.** En este repo un push a
`main` es un release a producción: CI testea, buildea, empuja a ECR y hace
`terraform apply -auto-approve` sobre ECS. Se hacen los cambios, se avisa, y
Saul decide cuándo se deployea.
