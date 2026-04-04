# Hedera Integration Analysis for x402-rs Facilitator

**Date**: 2026-04-03
**Context**: ETHGlobal Cannes 2026 — Hedera "$6K AI & Agentic Payments" prize
**Status**: Research complete — actionable paths identified

---

## TL;DR

| Capability | Status | Why |
|-----------|--------|-----|
| **x402 Payments (EIP-3009)** | BLOCKED | USDC on Hedera is HTS nativo, NO tiene `transferWithAuthorization()` |
| **ERC-8004 Reputation** | FUNCIONA | Contratos ya deployed en testnet, EVM compatible |
| **x402 via scheme alternativo** | POSIBLE | BlockyDevs ya tiene x402 en Hedera con "partially-signed transactions" |

**Recomendacion para el hackathon**: ERC-8004 on Hedera (funciona hoy) + x402 payments on otra chain (Base/Polygon) + demo multi-chain.

---

## 1. USDC en Hedera: Por que EIP-3009 NO funciona

### El problema fundamental

USDC en Hedera es un **token HTS nativo** (Hedera Token Service), NO un contrato Solidity ERC-20 estandar.

| Propiedad | Hedera (HTS) | Otras chains (ERC-20) |
|-----------|-------------|----------------------|
| Token ID | `0.0.456858` | N/A |
| EVM Address | `0x000000000000000000000000000000000006f89a` | Contract address normal |
| Standard | HTS nativo | FiatTokenV2_2.sol de Circle |
| `transferWithAuthorization()` | **NO** | **SI** |
| `permit()` (EIP-2612) | **NO** | **SI** |
| `DOMAIN_SEPARATOR()` | **NO** | **SI** |
| ERC-20 basico (transfer, approve, balanceOf) | SI (via facade HIP-218/376) | SI |
| Supply | ~$56.2M | Varies |

Circle desplegó USDC en Hedera como token HTS, no deployando su contrato `FiatTokenV2_2.sol`. Esto significa que las funciones EIP-3009 que nuestro facilitador necesita simplemente **no existen**.

### Testnet

| Red | Chain ID | USDC Token ID | EVM Address |
|-----|----------|--------------|-------------|
| Mainnet | 295 | `0.0.456858` | `0x000000000000000000000000000000000006f89a` |
| Testnet | 296 | `0.0.429274` | `0x0000000000000000000000000000000000068cda` |

Misma limitacion aplica en testnet.

### RPCs disponibles

| Red | RPC URL | Tipo |
|-----|---------|------|
| Mainnet | `https://mainnet.hashio.io/api` | Publico (rate limited) |
| Mainnet | `https://295.rpc.thirdweb.com` | Publico |
| Testnet | `https://testnet.hashio.io/api` | Publico |
| Premium | QuickNode, Arkhia | Requiere API key |

### Fuentes
- [Circle: USDC on Hedera](https://www.circle.com/multi-chain-usdc/hedera)
- [Circle: USDC Contract Addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses)
- [Hedera ERC-20 Facade (HIP-218/376)](https://docs.hedera.com/hedera/core-concepts/smart-contracts/tokens-managed-by-smart-contracts/erc-20-fungible-tokens)

---

## 2. ERC-8004 en Hedera: SI funciona

### Por que funciona

ERC-8004 es un contrato Solidity puro (ERC-721 + logica de reputacion). No depende de USDC ni de EIP-3009. Hedera ejecuta contratos Solidity en su EVM (basado en Hyperledger Besu).

### Contratos ya desplegados

Los contratos ERC-8004 ya existen en Hedera testnet en las direcciones deterministicas CREATE2:

| Contrato | Testnet | Mainnet (pending) |
|----------|---------|-------------------|
| IdentityRegistry | `0x8004A818BFB912233c491871b3d84c89A494BD9e` | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` |
| ReputationRegistry | `0x8004B663056A597Dffe9eCcC1965A193B7388713` | `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` |

### Funciones ERC-721 soportadas en Hedera

| Funcion | Status | Notas |
|---------|--------|-------|
| `ownerOf` | OK | Core de ERC-8004 |
| `approve` | OK | Via transaccion sintetica |
| `setApprovalForAll` | OK | Via transaccion sintetica |
| `transferFrom` | OK | Via transaccion sintetica |
| `name`, `symbol` | OK | Standard |
| `tokenURI` | OK | Metadata de agentes |
| `totalSupply` | OK | Standard |
| `balanceOf` | OK | Standard |
| `safeTransferFrom` | **LIMITADO** | HTS token association rules. Workaround: usar `transferFrom` |

**Para ERC-8004 esto es suficiente.** Las funciones core (register, feedback, reputation queries) no necesitan `safeTransferFrom`.

### Ecosistema complementario en Hedera

Hedera tiene su propio standard de identidad de agentes:

| Standard | Funcion | Relacion con ERC-8004 |
|----------|---------|----------------------|
| **HCS-14** (Universal Agent IDs) | Identificador portable cross-protocol | Complementario — envuelve ERC-8004 |
| **HCS-10** (OpenConvAI) | Comunicacion entre agentes | Capa de messaging |
| **Hedera Agent Kit** | SDK para construir agentes | Tooling |
| **HOL Registry** | Registro dual HCS-14 + ERC-8004 | Bridge entre ecosistemas |

**Hedera explicitamente diseño su ecosistema de agentes para ser complementario a ERC-8004, no competitivo.**

### Fuentes
- [ERC-8004 Contracts Repo](https://github.com/erc-8004/erc-8004-contracts)
- [Hedera EVM Smart Contracts](https://docs.hedera.com/hedera/core-concepts/smart-contracts/understanding-hederas-evm-differences-and-compatibility)
- [HCS-14: Universal Agent IDs](https://hol.org/blog/hcs-14-universal-agent-ids/)
- [Launch ERC-8004 Agent on HOL Registry](https://hol.org/blog/launch-erc-8004-agent-hol-registry/)

---

## 3. Paths de integracion posibles

### Path A: ERC-8004 Only (RECOMENDADO para hackathon)

**Esfuerzo**: ~2 horas | **Riesgo**: Bajo

Agregar Hedera como red soportada SOLO para ERC-8004 (reputacion/identidad de agentes), sin soporte de pagos x402.

**Que se necesita**:
1. Agregar `Network::Hedera` y `Network::HederaTestnet` a `src/network.rs` (sin USDC deployments)
2. Agregar addresses de contratos ERC-8004 a `src/erc8004/mod.rs`
3. Agregar configuracion RPC para Hedera
4. Demo: registrar agente en Hedera testnet, dar feedback, consultar reputacion

**Demo narrative**: "Nuestro facilitador soporta ERC-8004 agent reputation en Hedera, complementando HCS-14. Los pagos x402 funcionan en 22+ mainnets (Base, Polygon, etc.), y la reputacion del agente se sincroniza cross-chain."

**Pros**:
- Funciona HOY sin cambios al protocolo
- Encaja con lo que Hedera quiere ver (ERC-8004 + HCS-14 complementarios)
- Bajo riesgo, rapido de implementar

**Contras**:
- No hay pagos x402 *en* Hedera
- Menos impactante que un full integration

---

### Path B: x402 con scheme HTS nativo (POST-hackathon)

**Esfuerzo**: 2-4 semanas | **Riesgo**: Medio

Crear un nuevo payment scheme (`scheme_exact_hts`) que use transacciones parcialmente firmadas de Hedera en vez de EIP-3009.

**Como funciona (modelo BlockyDevs)**:
1. Cliente construye una transaccion HTS transfer
2. Cliente firma con su key
3. Envia la transaccion parcialmente firmada al facilitador
4. Facilitador co-firma y la envia a Hedera, pagando gas

**Que se necesita**:
- Nuevo `NetworkFamily` variant: `Hedera`
- Nuevo modulo: `src/chain/hedera.rs`
- Dependencia: Hedera SDK para Rust (o interactuar via JSON-RPC relay)
- Nuevos tipos: `HtsPaymentPayload`, verificacion de firmas parciales
- Nuevo scheme en la spec x402 (no existe upstream)

**Pros**:
- Full x402 payments en Hedera
- Gasless para el usuario (facilitador paga HBAR gas)

**Contras**:
- Scheme no estandar (no esta en la spec x402 upstream)
- 2-4 semanas de desarrollo
- Requiere auditar el flujo de co-firma

---

### Path C: USDC wrapper con EIP-3009 (NO recomendado)

**Esfuerzo**: 1-2 semanas | **Riesgo**: Alto

Deployar un contrato ERC-20 wrapper en Hedera EVM que envuelva USDC-HTS y agregue `transferWithAuthorization()`.

**Por que NO**:
- Agrega trust assumptions (usuarios deben confiar en el wrapper)
- Requiere wrap/unwrap de USDC (friccion)
- Gas costs adicionales
- No es como Circle intento que USDC funcione en Hedera
- Riesgo de seguridad del wrapper contract

---

### Path D: Multi-chain demo (PRAGMATICO para hackathon)

**Esfuerzo**: 0 horas adicionales | **Riesgo**: Cero

Usar el facilitador TAL CUAL ESTA para demostrar x402 payments en chains soportadas (Base, Polygon, etc.) + ERC-8004 en Hedera testnet.

**Demo narrative**: "El Execution Market usa x402 para pagos entre agentes en 22+ blockchains, con identidad verificable via ERC-8004. En Hedera, la reputacion del agente se registra on-chain complementando HCS-14/UAID, mientras los pagos settlan en la chain optima para el usuario."

**Pros**:
- Cero trabajo nuevo
- Demuestra la fortaleza real: multi-chain
- Hedera quiere ver x402 + ERC-8004 — los tenemos en el stack

**Contras**:
- Los pagos no settlan en Hedera directamente

---

## 4. Detalles del Hedera Prize (ETHGlobal Cannes)

### AI & Agentic Payments — $6,000 (2 x $3,000)

**Requisito core**: AI agent que ejecute al menos un pago/token transfer en Hedera Testnet.

**Tecnologias que explicitamente mencionan**:
- Hedera Agent Kit
- OpenClaw ACP
- **x402** (lo mencionan directamente)
- A2A protocol
- Hedera SDKs

**Enhancements opcionales que matchean nuestro stack**:
- x402 implementation para pay-per-request API access
- On-chain agent identity via **ERC-8004** o HCS-14
- Multi-agent payment negotiation
- Agent discovery via UCP

**Requisitos de submission**:
- Repo publico de GitHub con README
- Demo video (max 5 min) mostrando acciones autonomas de pago

### Fuentes
- [ETHGlobal Cannes Hedera Prize](https://ethglobal.com/events/cannes2026/prizes/hedera)
- [Hedera x402 Blog Post](https://hedera.com/blog/hedera-and-the-x402-payment-standard/)

---

## 5. Datos tecnicos de Hedera EVM

| Propiedad | Valor |
|-----------|-------|
| Chain ID mainnet | 295 |
| Chain ID testnet | 296 |
| EVM Implementation | Hyperledger Besu |
| Finality | ~2.5 segundos, **deterministica** (no probabilistica) |
| Gas pricing | Fijo en USD, convertido a HBAR |
| Costo por transaccion | Fracciones de centavo |
| CREATE2 | Soportado (HIP-329, desde v0.23.0) |
| ecrecover | Soportado para ECDSA (secp256k1) |
| ED25519 keys | Soportado nativamente, pero NO compatible con ecrecover |
| Block explorer | hashscan.io |
| Tooling compatible | ethers.js, viem, Hardhat, Foundry, alloy |

### Diferencias clave vs EVM estandar
- `stateRoot` retorna empty Merkle trie root
- `COINBASE` retorna fee collection account `0.0.98`
- `safeTransferFrom` de HTS tokens tiene limitaciones (token association rules)
- Gas pricing fijo, no basado en subastas

---

## 6. Recomendacion final

### Para el hackathon (HOY)

**Path A + Path D combinados:**

1. Agregar Hedera testnet como red ERC-8004 en el facilitador (~2h)
2. Demo multi-chain: pagos x402 en Base/Polygon + reputacion ERC-8004 en Hedera
3. Integrar con HCS-14/UAID para mostrar interoperabilidad
4. Narrative: "x402 multi-chain payments + ERC-8004 reputation, with Hedera as the trust layer"

### Para post-hackathon (roadmap)

1. **Corto plazo**: Monitorear si Circle despliega FiatTokenV2 en Hedera EVM (habilitaria EIP-3009)
2. **Mediano plazo**: Evaluar Path B (scheme HTS nativo) si hay demanda
3. **Largo plazo**: Contribuir `scheme_exact_hts` al spec x402 upstream

---

## 7. Decision matrix

| Criterio | Path A (ERC-8004 only) | Path B (HTS scheme) | Path C (Wrapper) | Path D (Multi-chain demo) |
|----------|----------------------|--------------------|-----------------|-----------------------|
| Tiempo | 2h | 2-4 semanas | 1-2 semanas | 0h |
| Riesgo | Bajo | Medio | Alto | Cero |
| Impacto en hackathon | Alto | N/A (no da tiempo) | N/A (no da tiempo) | Medio |
| Pagos en Hedera | No | Si | Si | No |
| ERC-8004 en Hedera | Si | Si | Si | No |
| Estandar x402 compliant | Si | No (scheme custom) | Si | Si |
| Esfuerzo post-hackathon | Poco | Mucho | Mucho | Ninguno |

---

*Documento generado para decision-making del hackathon ETHGlobal Cannes 2026.*
*Research realizado el 2026-04-03 por claude-facilitator.*
