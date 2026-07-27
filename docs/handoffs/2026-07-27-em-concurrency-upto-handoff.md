# Handoff — concurrencia de settles, pineo de firmantes y UPTO (2026-07-27)

**Estado**: el trabajo está **cerrado y en producción**. Este documento existe para retomar
los pendientes, no para terminar algo a medias.

**Origen**: `docs/HANDOFF-2026-07-24-signer-pool-concurrencia.md` (pedido de Saul) + backlog de
Execution Market (filas 2026-07-07 #49/#50, 07-20 #194, 07-21 #10, 07-24 #224) +
`INC-2026-07-06` e `INC-2026-07-21` del repo EM.

**Análisis completo** (no repetido aquí):
- `docs/plans/em-concurrency-response-2026-07-24.md` — carril de nonces, 11 gaps, secuencia
- `docs/plans/upto-blockers-and-single-writer-2026-07-26.md` — matriz UPTO, condiciones de
  operators, opciones de infraestructura y por qué se descartaron

---

## 1. Corrección al pedido original

El handoff pedía **construir un signer pool**. El pool **ya existía y funcionaba** (upstream
PR #16): `next_signer_address()` en `src/chain/evm.rs`, y `EVM_PRIVATE_KEY_MAINNET` ya acepta
lista separada por comas. Encenderlo no requiere Terraform ni IAM.

Los bloqueadores reales estaban en otro lado. No re-implementar el pool.

## 2. Qué se desplegó

| Versión | Contenido |
|---|---|
| **v1.58.0** | Carril de nonces: marca de agua que impide rebobinar bajo TX en vuelo; eliminado el bypass de Ethereum L1; guard pre/post con probes falibles; `RetryBackoffLayer` para rate limits; `is_nonce_error` ampliado; reintentos 1→2 con jitter |
| **v1.59.0** | **P0**: `send_transaction` no chequeaba `receipt.status()` — un `release` revertido volvía como `success:true`. Pineo de UPTO / PaymentOperator / ERC-8004. Lease de escritor único |
| **v1.59.1** | UPTO rechaza witnesses inválidos antes de gastar RPC |

Antes de esto también se arregló el **owner-scan de ERC-8004** (`-32003 out of gas`), que
llevaba 19 días roto en Base y era el amplificador de mints duplicados. Salió dentro del
commit `8f3990e4` de otra sesión que corría en paralelo.

## 3. Hechos probados (no inferidos) — no volver a investigar

**UPTO**. El proxy `0x4020A4f3b7b90ccA423B9fabCc0CE57C6C240002` aplica
`msg.sender == witness.facilitator` **incondicionalmente**. Fuente confirmada vía Sourcify:

```solidity
if (msg.sender != witness.facilitator) revert UnauthorizedFacilitator();
```

`Address::ZERO` **NO es comodín** — revierte igual (`0x0f6fae87`). Y como `facilitator` está
dentro del typehash EIP-712, el pagador se compromete a una address al firmar: **rotar UPTO
entre un pool es un cambio de protocolo, no de scheduling**.

**PaymentOperator**. Las 8 mainnets legacy alcanzables pinean el EOA canónico en
`RELEASE_CONDITION` y `REFUND_IN_ESCROW_CONDITION`, con bytecode de condición byte-idéntico.
Probado por diferencial en Optimism: desde otra address da `0x741926a1 ConditionNotMet()`.
`authorize` es el único agnóstico (916 de 1213 settles/30d).

**ERC-8004**. El feedback está indexado por `(agentId, msg.sender)`. **Nunca rotar**: partiría
la identidad del attester y fragmentaría `getSummary` para todos los consumidores.

**G1 (multi-writer)**. Exposición 100% por deploys, no por autoscaling — el servicio nunca
escaló. `minimumHealthyPercent=100 / maximumPercent=200` deja 2 tasks ~51-61s, ~32 veces/mes.

**Costo AWS real: $190.64/mes.** `CLAUDE.md` y `docs/LAMBDA_MIGRATION.md` dicen ~$43-48 —
están mal por **4.2x**. Mayor rubro: NAT $93.71 (horas + processing + egress).

## 4. Prueba end-to-end con dinero real (Base mainnet)

```
tx      0x52e93c878bc3e0337e1741f30646d0671c92e02cf9a858f2f869eca62fdab573
from    0x1030…13C7 (signer pineado)   to  0x4020A4f3…0002 (proxy)
status  1    gas 99.566    block 49152179
```

0.01 USDC cobrados de 0.03 autorizados (semántica "upto" verificada). Replay del mismo payload
rechazado por Permit2 con `0x756688fe InvalidNonce()`. Los tres negativos (facilitator
equivocado / omitido / ZERO) rechazados antes de gastar RPC.

### Receta para repetirlo (importa)

**No mandes TXs externas desde la wallet del facilitador mientras está en producción.** Durante
la prueba, un envío desde `0x1030…` chocó con `replacement transaction underpriced` porque el
facilitador vivo firma desde esa misma wallet: te conviertes en el segundo escritor.

Camino seguro, ya validado:
1. Pagador **efímero** (`Account.create()`), no la hot wallet
2. Fondearlo con **una sola** TX del facilitador, con `nonce=pending` y
   `maxFeePerGas = baseFee*2 + prio` (en Base `gas_price*4` queda **por debajo** del priority
   y devuelve `-32003`)
3. El efímero hace el `approve` a Permit2 (sin contención)
4. Al terminar, barrer el USDC de vuelta y destruir la llave

Así la hot wallet nunca otorga un approval de Permit2. EIP-712: dominio
`{name:"Permit2", chainId, verifyingContract}` **sin** campo `version`.

---

## 5. Pendientes

Ordenados por lo que necesita decisión versus lo que es puro trabajo.

### 5.1 Necesitan tu decisión

**¿Encender el pool (N > 1)?**
Está desbloqueado pero con 1 llave, deliberadamente. Cuesta **$0** en Secrets Manager (ya es
lista separada por comas en un secreto existente). Lo que cuesta es operarlo: fondear y
monitorear ~20 EOAs por chain. Recomendación: **medir contención real primero**. Antes de
encenderlo hay que auditar on-chain `RELEASE_CONDITION()` / `REFUND_IN_ESCROW_CONDITION()` de
los 10 operators desplegados por merchants.

**NAT Gateway → instancia (~$58/mes de ahorro).**
Es el ahorro más grande disponible, pero introduce un punto único de falla para todo el egress
y necesita mantenimiento de AMI. Merece su propia revisión de riesgo. **No bundlearlo** con
otro cambio.

### 5.2 Trabajo pendiente, sin decisiones abiertas

**Cinco redes mienten en `/supported`.** Anunciamos UPTO en `avalanche`, `celo`, `unichain`,
`scroll`, `optimism-sepolia` donde el proxy **no tiene código**. Ahí siempre va a fallar. Hay
que dejar de anunciarlas o desplegar el proxy. Redes con proxy confirmado (3142 bytes,
keccak prefix `4662dc27323421a3`): base, optimism, arbitrum, bsc, ethereum, polygon, hyperevm,
monad, base-sepolia, avalanche-fuji, arbitrum-sepolia. Sin probar: robinhood (4663/46630),
hyperevm-testnet, unichain-sepolia, polygon-amoy, celo-alfajores, skale.

**Corregir los docs de costo.** `CLAUDE.md` y `docs/LAMBDA_MIGRATION.md:80-89` dicen ~$43-48.
Cualquier juicio de "¿esto es caro?" hecho con ese número arranca 4.2x torcido.

**Ahorros seguros ($25 de los $83.51), con precondiciones:**
- ECR sin lifecycle policy — $1.54 (310 imágenes, 15.36 GB)
- 5 alarmas muertas + 2 dashboards vacíos — $2.90 (`Facilitator/Protocol` y `Facilitator/NEAR`
  devuelven CERO métricas)
- VPC endpoint de Secrets Manager `vpce-031aa5860d878ffec` — $7.30 (vs $0.17/mes de uso real)
- Container Insights — $10.58 · **BLOQUEADO** hasta mover la alarma
  `facilitator-production-no-running-tasks` fuera de `ECS/ContainerInsights RunningTaskCount`,
  o te quedas sin la única señal de liveness
- 4 secretos deprecados — $2.00 · **BLOQUEADO** hasta quitar `EVM_PRIVATE_KEY` /
  `SOLANA_PRIVATE_KEY` del `secrets[]` de la task definition

### 5.3 Sin verificar (no darlo por bueno)

- **El lease nunca hizo handover real.** Está vivo y adquirido en producción, pero no se lo vio
  competir con dos tasks durante un deploy. Verificable: filtrar `"writer lease"` en
  `/ecs/facilitator-production` durante un deploy y confirmar Acquired/Lost/Released.
- **UPTO solo probado en Base.** Bytecode idéntico en otras 10 chains, ninguna ejercitada.
- **Escrituras ERC-8004 (`giveFeedback`, `revokeFeedback`, `appendResponse`, attestations)
  siguen salteando `send_transaction`**: sin reset de nonce y **sin timeout de receipt**. Al
  rutearlas hay que pinear el `from` explícitamente (ver §3, ERC-8004) o se rompe la revocación.

### 5.4 Devolver a Execution Market

1. Buena parte de sus **258 errores `-32007` era auto-infligida** por el owner-scan roto que ya
   se arregló. **Re-medir antes de comprar tier pago de RPC.**
2. Siguen pendientes los datos por chain que se les pidió (chain, count, error code, timestamp).
3. Su fila 2026-07-21 "(a) resolver por owner" está **cerrada**: el endpoint ya no devuelve 404
   ante fallo transitorio, ahora devuelve `503 + retryable:true`. Eso era lo que hacía que
   persistieran `agent_id=NULL`.

---

## 6. Trampas conocidas

- **No usar `signer_addresses[0]`** como ancla del pineo — es orden de iteración de hash. Usar
  `default_signer_address()` (expuesto como `pinned_signer()`).
- **No validar cambios de pool en base-sepolia / ethereum-sepolia**: esos operators tienen todas
  las condiciones en `address(0)`, nunca reproducen el bug.
- **No correr `terraform apply` completo** — re-sube la Lambda de balances. Usar `-target`.
- **`Cargo.lock` tiene que sincronizarse a mano** en cada bump (CI usa `--locked`).
- **Otra sesión puede estar editando el mismo árbol.** Pasó en esta: los cambios de
  `handlers.rs` terminaron dentro del commit `8f3990e4` de la sesión `bazaar-enhancer`.
  Verificar `git status` antes de asumir que el working tree es tuyo.

## 7. Estado al cierre

- Producción: **v1.59.2**, healthy
- Local: `v1.59.3` commiteado (`1e8b60c3`, trabajo de bazaar de otra línea) — **sin desplegar**
- Todo lo de este handoff está commiteado y desplegado
- Kill-switch disponible: `ENABLE_WRITER_LEASE=false`
