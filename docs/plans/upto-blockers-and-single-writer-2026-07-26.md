# Bloqueadores de UPTO, pineo de firmantes y escritor único (2026-07-26)

Investigación con 5 equipos en paralelo + verificación adversarial, más pruebas directas
contra los contratos desplegados. Todo lo marcado **PROBADO** se verificó con `eth_call`
real o leyendo bytecode, no por inferencia.

---

## 1. UPTO — matriz completa de escenarios (PROBADO on-chain)

El proxy `X402UptoPermit2Proxy` (`0x4020A4f3b7b90ccA423B9fabCc0CE57C6C240002`) aplica
`msg.sender == witness.facilitator` de forma **incondicional**. El selector
`UnauthorizedFacilitator()` = `0x0f6fae87` está presente en el bytecode desplegado en Base
(3142 bytes).

| `witness.facilitator` | `msg.sender` | Resultado | Prueba |
|---|---|---|---|
| ZERO | canónico | `0x0f6fae87` ❌ | eth_call Base |
| ZERO | otro | `0x0f6fae87` ❌ | eth_call Base |
| canónico | otro | `0x0f6fae87` ❌ | eth_call Base |
| canónico | canónico | pasa el gate → `0x4be6321b` `InvalidSignatureLength` de Permit2 ✅ | eth_call Base |
| otro | otro | pasa el gate ✅ | eth_call Base + publicnode |

**`Address::ZERO` NO es comodín.** Las dos últimas filas prueban que compara contra el
witness, no contra una address inmutable.

Orden de las validaciones internas (importa para diagnosticar): `AmountExceedsPermitted`
corre **antes** del gate de facilitator; `PaymentTooEarly` e `InvalidAmount` corren
**después**. Un bug de rotación siempre se presenta como `0x0f6fae87`, nunca como un error
posterior.

**Por qué no se puede rotar**: `facilitator` está dentro del typehash EIP-712
(`WITNESS_TYPEHASH` = `keccak("Witness(address to,address facilitator,uint256 validAfter)")`),
así que el pagador se compromete con **una** address al firmar. Reescribir el calldata
rompería la firma. Habilitar un pool para UPTO sería un cambio de protocolo (publicar todas
las addresses y que el cliente firme para la que va a transmitir), no de scheduling.

**Decisión: pinear.** `witness.facilitator` ahora es obligatorio, debe ser exactamente el
signer pineado, y tanto la simulación como la transmisión usan esa address.

### Cobertura del proxy (PROBADO)
Tiene código en base, optimism, arbitrum, bsc, ethereum, polygon, hyperevm, monad,
base-sepolia, avalanche-fuji, arbitrum-sepolia. **No tiene código** en avalanche, celo,
unichain, scroll, optimism-sepolia — 5 redes que `/supported` anuncia y que nunca podrán
liquidar UPTO. Pendiente decidir si se dejan de anunciar.

## 2. PaymentOperator — pineado en TODAS las mainnets (PROBADO)

No es solo SKALE. En las 8 mainnets legacy alcanzables (Base, Ethereum, Polygon, Arbitrum,
Celo, Monad, Avalanche, Optimism):

- `REFUND_IN_ESCROW_CONDITION` = `StaticAddressCondition(0x103040545AC5031A11E8C03dd11324C7333a13C7)`
- `RELEASE_CONDITION` = `OrCondition([PayerCondition, StaticAddressCondition(0x1030…13C7)])`
- El bytecode de la condición es **byte-idéntico** en las 8 chains

Prueba diferencial en Optimism mainnet: `refundInEscrow` desde `0x1030…13C7` revierte dentro
del escrow (`0x3a4366b7`), mientras que desde cualquier otra address revierte con
`0x741926a1 = ConditionNotMet()`. Los 3 operators de SKALE Base pinean lo mismo.

`authorize` es el único agnóstico al caller (916 de 1213 settles de escrow en 30 días).
`DepositRelay` también es agnóstico y no tiene tráfico.

**Decisión: pinear los tres** (`authorize` incluido). Es seguro porque `authorize` no
chequea caller, y un modelo uniforme evita que alguien rote por accidente el que sí importa.

## 3. ERC-8004 — nunca rota

El feedback está indexado por `(agentId, msg.sender)`: el mismo índice se revoca bien desde
el autor y revierte "index out of bounds" desde cualquier otro. `getSummary` además revierte
con "clientAddresses required" ante un filtro vacío, así que una identidad de attester
partida fragmenta silenciosamente la vista de todos los consumidores.

Estas escrituras siguen usando el signer por defecto y **no** se rutearon por el pool.

## 4. El P0 que nadie había visto

`send_transaction` **nunca chequeaba `receipt.status()`** — solo la ruta EIP-3009 lo hacía.
Un `release`/`refundInEscrow` revertido volvía al merchant como `success: true` con hash real.
~297 llamadas/mes en 9 mainnets expuestas. Es la misma clase de defecto (FAC-1) que se
arregló para ERC-8004 en v1.49.0 y que seguía abierta en todos los demás caminos.

**Arreglado dentro de `send_transaction_from`**, no en cada call site.

## 5. Auto-DoS del nonce (corrige mi propio cambio de v1.58.0)

Alloy rellena gas y nonce **concurrentemente** (`try_join!` en `JoinFill::prepare`), y
`NonceFiller::prepare` consume el nonce apenas corre. Si la estimación revierte después, el
nonce quedó gastado por una TX que nunca se transmite. La marca de agua que introduje en
v1.58.0 convertía ese hueco auto-sanable en un bloqueo de hasta 120s — y `/settle` no está
autenticado, así que una ráfaga de payloads inválidos podía frenar settles legítimos.

**Arreglado**: se estima gas **antes** de reservar el nonce (un revert ya no cuesta nada), y
las fallas que provablemente no llegaron al mempool devuelven el nonce con `release_nonce`.

## 6. G1 — escritor único

Exposición medida: **100% por deploys**, no por autoscaling (el servicio nunca escaló;
el scale-out alarm no cambia de estado desde 2025-12-04). Cada deploy rolling deja 2 tasks
~51-61s, ~32 veces al mes.

Opciones evaluadas:

| Opción | Veredicto |
|---|---|
| (a) `maximumPercent=100` | Provablemente sin solape, **pero ~130s de caída dura por deploy** (~50 min/mes de 5xx). Descartada. |
| (b) Nonces en DynamoDB | Convierte una carrera en un atasco: una TX caída deja un hueco permanente. Descartada. |
| (c) Sharding por índice de task | **Imposible**: ECS no expone ordinal; `ECS_CONTAINER_METADATA_URI_V4` da un TaskARN aleatorio. Ambas tasks calcularían índice 0. Descartada. |
| **(d) Lease de escritor único** | **Elegida.** Cero cambios de Terraform, cero downtime. |

El lease usa `PutItem` condicional sobre la tabla `facilitator-nonces` que ya existe
(PAY_PER_REQUEST, TTL, VPC gateway endpoint) — y `dynamodb:PutItem` ya está en la política
IAM, así que un put condicional no requiere permisos nuevos.

- TTL 15s, renovación cada 5s, `DeleteItem` en apagado para handover inmediato
- El no-titular sigue sirviendo lecturas y solo rechaza escrituras EVM
- **Fail-open**: si DynamoDB no responde, se asume el rol de escritor y se loguea fuerte —
  degradar al comportamiento anterior es preferible a dejar de cobrar
- Kill-switch `ENABLE_WRITER_LEASE=false`

## 7. Qué NO se hizo, deliberadamente

- **No se añadieron llaves al pool.** El pool queda *desbloqueado pero sin usar*: la decisión
  de fondear N EOAs en 20 chains debe tomarse con un número medido de contención, no a ciegas.
- **No se tocó el NAT gateway → NAT instance** (~$58/mes de ahorro): es el cambio más grande
  pero introduce un punto único de falla para todo el egress y merece revisión aparte.
- **No se bajó el Fargate a 0.5 vCPU**: el pico de 1 minuto en 30 días fue 91.76% de 1 vCPU.
- **No se corrió `terraform apply` completo** (re-sube la Lambda de balances).
- **No se validó en testnets**: los operators de base-sepolia y ethereum-sepolia tienen todas
  las condiciones en `address(0)`, así que nunca reproducirían el bug.
