# Session Handoff: Multi-Stablecoin Implementation

## Proyecto
x402-rs Payment Facilitator - Ultravioleta DAO
Ruta WSL: /mnt/z/ultravioleta/dao/x402-rs

## Estado Actual
Solo soporta USDC. Queremos agregar EURC, AUSD, PYUSD, GHO, crvUSD.

## Matriz Verificada (EIP-3009)

| Token | Networks | Decimals |
|-------|----------|----------|
| USDC | 10/10 EVM | 6 |
| EURC | Ethereum, Base, Avalanche | 6 |
| AUSD | Ethereum, Polygon, Arbitrum, Avalanche, Monad | 6 |
| PYUSD | Ethereum | 6 |
| GHO | Ethereum, Arbitrum, Base | 18 |
| crvUSD | Ethereum, Arbitrum | 18 |

## Direcciones de Contratos

EURC:
- Ethereum: 0x1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c
- Base: 0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42
- Avalanche: 0xC891EB4cbdEFf6e073e859e987815Ed1505c2ACD

AUSD (CREATE2 - same address all chains):
- 0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a

PYUSD:
- Ethereum: 0x6c3ea9036406852006290770BEdFcAbA0e23A0e8

GHO:
- Ethereum: 0x40D16FC0246aD3160Ccc09B8D0D3A2cD28aE6C2f
- Arbitrum: 0x7dfF72693f6A4149b17e7C6314655f6A9F7c8B33
- Base: 0x6Bb7a212910682DCFdbd5BCBb3e28FB4E8da10Ee

crvUSD:
- Ethereum: 0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E
- Arbitrum: 0x498Bf2B1e120FeD3ad3D42EA2165E9b73f99C1e5

## 10 EVM Mainnets
Ethereum, Base, Polygon, Arbitrum, Optimism, Avalanche, Celo, HyperEVM, Unichain, Monad

## 4 Non-EVM
Solana, NEAR, Stellar, Fogo

## Plan de Implementacion (1070 lineas, 2-3 dias)

Fase 1: src/types.rs - TokenType enum (+80 lines)
Fase 2: src/network.rs - Token deployments (+300 lines)
Fase 3: src/chain/evm.rs - Validation (+20 lines)
Fase 4: src/handlers.rs - Multi-token /supported (+50 lines)
Fase 5: static/index.html - Token selector UI (+150 lines)
Fase 6: tests/ - Unit + integration (+300 lines)
Fase 7: Deploy Docker + ECS
Fase 8: Docs CHANGELOG + guide

## Consideraciones Criticas
- GHO y crvUSD usan 18 decimals (no 6)
- Cada token tiene EIP-712 domain diferente (name/version)
- tokenType es opcional, default = USDC (backward compat)
- USDT/DAI/FRAX NO soportan EIP-3009

## Documento de Referencia
docs/STABLECOIN_EXPANSION_PLAN.md (v1.3)

## Proximo Paso
Comenzar Fase 1: Agregar TokenType enum en src/types.rs
