# Plan de Merge: Upstream v0.11.2

**Fecha**: 2026-01-12
**Upstream Version**: v0.11.2 (x402-rs/x402-rs)
**Nuestra Version Actual**: v1.19.2 (basada en upstream v0.10.0)
**Autor**: Claude Code

---

## Resumen Ejecutivo

El upstream ha realizado una **refactorizacion arquitectonica MASIVA** entre v0.10.0 y v0.11.2:
- **+15,230 lineas agregadas**
- **-10,177 lineas eliminadas**
- **76 archivos modificados**

La refactorizacion introduce un nuevo sistema de "Schemes" y reorganiza completamente la estructura del codigo. **NO es un merge trivial** - requiere decisiones estrategicas sobre que adoptar y que mantener.

---

## Analisis Comparativo

### Nuestra Arquitectura Actual (UltravioletaDAO Fork)

```
src/
├── main.rs              # Entrypoint con Axum
├── handlers.rs          # 55KB - Landing page HTML + endpoints custom
├── network.rs           # 77KB - Network enum con 20+ networks
├── types.rs             # 73KB - Tipos x402 v1
├── types_v2.rs          # 52KB - Nuestra implementacion x402 v2
├── caip2.rs             # 16KB - CAIP-2 parsing
├── from_env.rs          # 21KB - Carga de configuracion desde env
├── provider_cache.rs    # Cache de RPC providers
├── facilitator.rs
├── facilitator_local.rs # 19KB
├── escrow.rs            # 33KB - Escrow settlements (CUSTOM)
├── nonce_store.rs       # 16KB - DynamoDB nonce store (CUSTOM)
├── discovery.rs         # 31KB - Meta-Bazaar discovery (CUSTOM)
├── discovery_aggregator.rs # 36KB (CUSTOM)
├── discovery_crawler.rs # 16KB (CUSTOM)
├── discovery_store.rs   # 15KB - S3 persistence (CUSTOM)
├── blocklist.rs         # 6KB - Address blocklist (CUSTOM)
├── fhe_proxy.rs         # 7KB - FHE proxy support (CUSTOM)
├── telemetry.rs
├── timestamp.rs
├── sig_down.rs
├── debug_utils.rs
└── chain/
    ├── mod.rs
    ├── evm.rs           # 75KB - EIP-3009 settlements
    ├── solana.rs        # 45KB
    ├── near.rs          # 27KB - NEP-366 (CUSTOM - no en upstream)
    ├── stellar.rs       # 59KB - Soroban (CUSTOM - no en upstream)
    ├── algorand.rs      # 36KB - Atomic groups (CUSTOM - no en upstream)
    └── sui.rs           # 22KB - Sponsored txns (CUSTOM - no en upstream)
```

### Arquitectura Upstream v0.11.2

```
src/
├── main.rs              # Simplificado, usa Config struct
├── handlers.rs          # Simple, texto plano "Hello from x402-rs!"
├── networks.rs          # Nuevo nombre, solo 12 networks conocidos
├── config.rs            # NUEVO - Configuracion JSON/CLI con clap
├── facilitator.rs
├── facilitator_local.rs # Usa SchemeRegistry
├── lib.rs               # Re-exports limpios
├── timestamp.rs
├── chain/
│   ├── mod.rs           # ChainRegistry, ChainProvider
│   ├── chain_id.rs      # NUEVO - CAIP-2 ChainId, ChainIdPattern
│   ├── eip155/          # MODULARIZADO
│   │   ├── mod.rs       # Eip155ChainProvider
│   │   ├── pending_nonce_manager.rs
│   │   └── types.rs
│   └── solana.rs        # Actualizado a solana-* v3/v4
├── proto/               # NUEVO - Tipos del protocolo
│   ├── mod.rs
│   ├── util.rs
│   ├── v1.rs
│   └── v2.rs
├── scheme/              # NUEVO - Sistema de schemes
│   ├── mod.rs           # SchemeRegistry, SchemeBlueprints
│   ├── client.rs        # PaymentSelector, PaymentCandidateLike
│   ├── v1_eip155_exact/
│   │   ├── mod.rs
│   │   ├── client.rs
│   │   └── types.rs
│   ├── v1_solana_exact/
│   │   ├── mod.rs
│   │   ├── client.rs
│   │   └── types.rs
│   ├── v2_eip155_exact/
│   │   ├── mod.rs
│   │   ├── client.rs
│   │   └── types.rs
│   └── v2_solana_exact/
│       ├── mod.rs
│       ├── client.rs
│       └── types.rs
└── util/                # NUEVO - Utilidades extraidas
    ├── mod.rs
    ├── b64.rs
    ├── lit_str.rs
    ├── money_amount.rs
    ├── sig_down.rs
    └── telemetry.rs
```

---

## Diferencias Clave

### 1. Networks/Chains Soportados

| Network | Nosotros | Upstream v0.11.2 |
|---------|----------|------------------|
| Base (mainnet/sepolia) | SI | SI |
| Polygon (mainnet/amoy) | SI | SI |
| Avalanche (mainnet/fuji) | SI | SI |
| Solana (mainnet/devnet) | SI | SI |
| Sei (mainnet/testnet) | NO | SI |
| XDC | NO | SI |
| XRPL EVM | NO | SI |
| Peaq | NO | SI |
| IoTeX | NO | SI |
| **Optimism** | SI | **NO** |
| **Celo** | SI | **NO** |
| **HyperEVM** | SI | **NO** |
| **Arbitrum** | SI | **NO** |
| **Ethereum mainnet** | SI | **NO** |
| **Unichain** | SI | **NO** |
| **Monad** | SI | **NO** |
| **MegaETH** | SI | **NO** |
| **World Chain** | SI | **NO** |
| **NEAR Protocol** | SI (completo) | **NO** |
| **Stellar/Soroban** | SI (completo) | **NO** |
| **Algorand** | SI (feature) | **NO** |
| **Sui** | SI (feature) | **NO** |

**Conclusion**: Nosotros tenemos MAS networks que el upstream. No debemos perder ninguno.

### 2. Features Exclusivas Nuestras (NO existen en upstream)

| Feature | Archivos | Descripcion |
|---------|----------|-------------|
| **Meta-Bazaar Discovery** | `discovery*.rs`, `discovery_store.rs` | Sistema de descubrimiento de facilitadores |
| **Escrow Settlements** | `escrow.rs` | Settlements via escrow contracts |
| **DynamoDB Nonce Store** | `nonce_store.rs` | Nonces persistentes en DynamoDB |
| **Address Blocklist** | `blocklist.rs` | Lista negra de direcciones |
| **FHE Proxy** | `fhe_proxy.rs` | Soporte para Fully Homomorphic Encryption |
| **NEAR Protocol** | `chain/near.rs` | NEP-366 meta-transactions |
| **Stellar/Soroban** | `chain/stellar.rs` | Token transfers en Soroban |
| **Algorand** | `chain/algorand.rs` | Atomic transaction groups |
| **Sui** | `chain/sui.rs` | Sponsored transactions |
| **Landing Page HTML** | `handlers.rs` + `static/` | Branding Ultravioleta DAO |
| **AWS Secrets Manager** | Integrado en `from_env.rs` | Carga de secretos desde AWS |

### 3. Implementacion x402 v2

| Aspecto | Nosotros | Upstream |
|---------|----------|----------|
| **Archivo tipos** | `types_v2.rs` (52KB) | `proto/v2.rs` (5KB) |
| **CAIP-2 parsing** | `caip2.rs` (16KB) | `chain/chain_id.rs` |
| **Conversion v1<->v2** | `PaymentRequirementsV1ToV2` trait | Traits en `scheme/` |
| **Handlers unificados** | En `handlers.rs` | Separados por scheme |
| **ResourceInfo** | Completo | Completo |
| **PaymentPayload v2** | Custom implementation | Generic `<TAccepted, TPayload>` |

**Nuestra implementacion v2 es MAS integrada** - soporta conversion bidireccional y esta embedded en los handlers existentes. La del upstream es mas modular pero menos completa.

### 4. Sistema de Configuracion

| Aspecto | Nosotros | Upstream |
|---------|----------|----------|
| **Fuente** | `.env` + AWS Secrets Manager | JSON config + CLI (clap) |
| **Archivo** | `from_env.rs` | `config.rs` |
| **RPC URLs** | Env vars individuales | `ChainsConfigMap` en JSON |
| **Private Keys** | Env vars o AWS Secrets | `LiteralOrEnv<T>` wrapper |
| **Features** | AWS Secrets Manager integrado | Solo env vars |

El sistema del upstream es mas limpio pero **NO tiene integracion con AWS Secrets Manager**.

---

## Mejoras del Upstream que Deberiamos Adoptar

### Alta Prioridad

1. **Nuevo sistema de nonce management** (`pending_nonce_manager.rs`)
   - Manejo mas robusto de nonces pendientes
   - Evita nonce collisions en alta concurrencia
   - **Beneficio**: Menos transacciones fallidas

2. **BlobGasFiller fix** (commit `6b0cb3f`)
   - Fix para inicializar BlobGasFiller usando default constructor
   - **Beneficio**: Compatibilidad con EIP-4844 blob transactions

3. **SPL token no-entrypoint features** (commit `6d260e7`)
   - Evita conflictos de symbols en Solana
   - **Beneficio**: Builds mas limpios

4. **Hyphen-separated scheme IDs** (commit `bde0733`)
   - Nuevo formato: `v1-eip155-exact` en lugar de `v1_eip155_exact`
   - **Beneficio**: Consistencia con especificacion x402 v2

5. **Solana mint verification** (commit `f672db2`)
   - Verifica que el mint del token match con el asset requerido
   - **Beneficio**: Seguridad mejorada

### Media Prioridad

6. **TTL cache para /supported endpoint** (commit `6a02d1b`)
   - Cache con TTL para respuestas de facilitadores
   - **Beneficio**: Menos llamadas RPC

7. **Telemetry mejorada en x402-reqwest** (commits `76a5b99`, `9c60bcf8`)
   - Spans de tracing para HTTP requests
   - **Beneficio**: Mejor observabilidad

8. **Dynamic pricing support** (commits `26ebbec` - `e3065f8`)
   - PriceTagSource trait para pricing dinamico
   - **Beneficio**: Flexibilidad en pricing

### Baja Prioridad (Nice to Have)

9. **Documentacion mejorada** (commit `3860295`)
   - Doc comments comprehensivos en todos los modulos
   - **Beneficio**: Mejor mantenibilidad

10. **Alloy crates desagregados**
    - Usa `alloy-primitives`, `alloy-provider`, etc. por separado
    - **Beneficio**: Builds mas rapidos, menos dependencias

---

## Lo que NO Deberiamos Adoptar

1. **Eliminacion de network.rs**
   - Upstream elimino `network.rs` en favor de `networks.rs` simplificado
   - Perderiamos todas nuestras redes custom (HyperEVM, Celo, Optimism, etc.)
   - **Mantener**: Nuestro `network.rs` de 77KB

2. **Sistema de configuracion JSON**
   - El `config.rs` del upstream no tiene AWS Secrets Manager
   - **Mantener**: Nuestro `from_env.rs` con integracion AWS

3. **Handlers simplificados**
   - El upstream usa texto plano "Hello from x402-rs!"
   - **Mantener**: Nuestra landing page HTML y branding

4. **Eliminacion de chains custom**
   - Upstream no tiene NEAR, Stellar, Algorand, Sui
   - **Mantener**: Todos nuestros `chain/*.rs` custom

5. **Edition 2024**
   - Upstream usa `edition = "2024"` (requiere Rust 1.86+)
   - **Mantener**: `edition = "2021"` para compatibilidad con Rust 1.82

---

## Estrategia de Merge Recomendada

### Opcion A: Cherry-Pick Selectivo (RECOMENDADO)

En lugar de hacer un merge completo, hacer cherry-pick de commits especificos que nos beneficien:

```bash
# Commits de alta prioridad para cherry-pick
git cherry-pick 6b0cb3f  # BlobGasFiller fix
git cherry-pick 6d260e7  # SPL token no-entrypoint
git cherry-pick f672db2  # Solana mint verification
git cherry-pick bde0733  # Hyphen-separated IDs (evaluar conflictos)
```

**Pros**:
- Control total sobre que cambios incorporar
- Sin riesgo de perder customizaciones
- Mas rapido de ejecutar

**Contras**:
- No obtenemos la arquitectura nueva de schemes
- Debemos seguir manteniendo nuestra estructura divergente

### Opcion B: Merge Parcial por Modulos

Hacer merge selectivo de modulos especificos:

1. **Merge `src/util/`** (nuevo)
   - Utilidades extraidas que no tenemos
   - Bajo riesgo de conflictos

2. **Merge `src/chain/eip155/`** (parcial)
   - Adoptar `pending_nonce_manager.rs`
   - Mantener nuestro codigo EVM existente
   - Requiere adaptacion

3. **NO merge de**:
   - `src/handlers.rs`
   - `src/networks.rs`
   - `src/config.rs`
   - `src/proto/`
   - `src/scheme/`

### Opcion C: Rebase Arquitectonico (NO RECOMENDADO)

Rehacer nuestra arquitectura para seguir la del upstream.

**Contras**:
- Esfuerzo ENORME (semanas de trabajo)
- Alto riesgo de regresiones
- Perdida de features custom
- No vale la pena dado que tenemos MAS funcionalidad

---

## Plan de Implementacion Detallado

### Fase 1: Preparacion (1-2 horas)

1. **Backup completo**
   ```bash
   cp -r /mnt/z/ultravioleta/dao/x402-rs /mnt/z/ultravioleta/dao/x402-rs-backup-pre-0.11.2
   ```

2. **Crear branch de trabajo**
   ```bash
   git checkout -b feature/upstream-0.11.2-selective-merge
   ```

3. **Documentar estado actual**
   - Correr tests: `cargo test`
   - Verificar build: `cargo build --release`
   - Guardar output de `/supported` endpoint

### Fase 2: Cherry-Picks de Alta Prioridad (2-4 horas)

1. **BlobGasFiller fix**
   ```bash
   git cherry-pick 6b0cb3f
   # Resolver conflictos en src/chain/evm.rs si los hay
   ```

2. **SPL token no-entrypoint**
   ```bash
   # Manual: Editar Cargo.toml
   # Cambiar spl-token y spl-token-2022 para agregar features = ["no-entrypoint"]
   ```

3. **Solana mint verification**
   ```bash
   git cherry-pick f672db2
   # Adaptar a nuestro src/chain/solana.rs
   ```

4. **Dependencias Solana actualizadas**
   - Evaluar si podemos actualizar a solana-* v3.x
   - Requiere testing exhaustivo

### Fase 3: Adopcion de Mejoras Selectivas (4-8 horas)

1. **Pending Nonce Manager**
   - Crear `src/chain/pending_nonce_manager.rs`
   - Extraer logica del upstream
   - Integrar con nuestro `chain/evm.rs`

2. **TTL Cache para /supported**
   - Implementar cache con TTL en `handlers.rs`
   - Usar DashMap con timestamp

3. **Scheme ID format**
   - Actualizar IDs de `v1_eip155_exact` a `v1-eip155-exact`
   - Mantener backward compatibility (aceptar ambos formatos)

### Fase 4: Testing y Validacion (2-4 horas)

1. **Unit tests**
   ```bash
   cargo test
   ```

2. **Integration tests**
   ```bash
   cd tests/integration
   python test_facilitator.py
   python test_usdc_payment.py --network base-sepolia
   ```

3. **Verificar customizaciones**
   ```bash
   # Landing page
   curl http://localhost:8080/ | grep "Ultravioleta"

   # Networks custom
   curl http://localhost:8080/supported | jq '.[] | select(.network | contains("hyperevm"))'
   curl http://localhost:8080/supported | jq '.[] | select(.network | contains("celo"))'
   curl http://localhost:8080/supported | jq '.[] | select(.network | contains("optimism"))'
   ```

4. **Verificar v2 protocol**
   ```bash
   # Test v2 verify endpoint
   curl -X POST http://localhost:8080/verify \
     -H "Content-Type: application/json" \
     -d '{"x402Version": 2, ...}'
   ```

### Fase 5: Documentacion y Cleanup (1-2 horas)

1. **Actualizar CUSTOMIZATIONS.md**
   - Documentar nuevos cherry-picks
   - Actualizar version base upstream

2. **Actualizar CHANGELOG.md**
   ```markdown
   ## v1.20.0 - 2026-01-XX

   ### Added
   - Pending nonce manager from upstream v0.11.2
   - TTL cache for /supported endpoint
   - Solana mint verification

   ### Changed
   - Updated scheme ID format to hyphen-separated
   - SPL token dependencies with no-entrypoint feature

   ### Fixed
   - BlobGasFiller initialization
   ```

3. **Bump version**
   ```bash
   # En Cargo.toml
   version = "1.20.0"
   ```

---

## Commits del Upstream para Cherry-Pick

### Prioridad Alta (HACER)

| Commit | Descripcion | Riesgo |
|--------|-------------|--------|
| `6b0cb3f` | BlobGasFiller default constructor | Bajo |
| `6d260e7` | SPL token no-entrypoint | Bajo |
| `f672db2` | Solana mint verification | Medio |
| `bde0733` | Hyphen-separated scheme IDs | Medio |

### Prioridad Media (EVALUAR)

| Commit | Descripcion | Riesgo |
|--------|-------------|--------|
| `6a02d1b` | TTL cache for /supported | Bajo |
| `76a5b99` | Telemetry spans en x402-reqwest | Bajo |
| `ef33047` | Revert nonce on transaction errors | Medio |

### No Aplicar

| Commit | Razon |
|--------|-------|
| `105f0ab` | Elimina .env.example (necesitamos el nuestro) |
| `a62ff07`+ | Nueva arquitectura de schemes (incompatible) |
| Todos los `wip` commits | Work in progress, inestable |

---

## Archivos PROTEGIDOS (NUNCA sobrescribir)

```
NUNCA TOCAR DESDE UPSTREAM:
├── static/index.html              # Landing page
├── static/images/*                # Logos
├── src/handlers.rs                # Custom endpoints + landing page
├── src/network.rs                 # Nuestras redes custom
├── src/from_env.rs                # AWS Secrets Manager
├── src/escrow.rs                  # Feature custom
├── src/nonce_store.rs             # Feature custom
├── src/discovery*.rs              # Meta-Bazaar
├── src/blocklist.rs               # Feature custom
├── src/fhe_proxy.rs               # Feature custom
├── src/types_v2.rs                # Nuestra implementacion v2
├── src/caip2.rs                   # Nuestro CAIP-2
├── src/chain/near.rs              # Chain custom
├── src/chain/stellar.rs           # Chain custom
├── src/chain/algorand.rs          # Chain custom
├── src/chain/sui.rs               # Chain custom
└── Cargo.toml                     # Nuestras dependencias custom
```

---

## Riesgos y Mitigaciones

| Riesgo | Probabilidad | Impacto | Mitigacion |
|--------|--------------|---------|------------|
| Perdida de landing page | Media | Alto | Backup antes de merge |
| Perdida de networks custom | Media | Alto | Nunca merge network.rs |
| Regresion en EVM settlements | Media | Alto | Tests exhaustivos |
| Incompatibilidad Solana | Media | Medio | Test en devnet primero |
| Breaking change en API | Baja | Alto | Versionamiento de endpoints |

---

## Timeline Estimado

| Fase | Duracion | Dependencias |
|------|----------|--------------|
| Preparacion | 1-2 horas | Ninguna |
| Cherry-picks alta prioridad | 2-4 horas | Fase 1 |
| Mejoras selectivas | 4-8 horas | Fase 2 |
| Testing | 2-4 horas | Fase 3 |
| Documentacion | 1-2 horas | Fase 4 |
| **Total** | **10-20 horas** | |

---

## Conclusion

**Recomendacion Final**: Opcion A (Cherry-Pick Selectivo)

El upstream ha divergido significativamente con su refactorizacion de schemes. Intentar un merge completo seria:
1. Extremadamente complejo
2. Alto riesgo de perder funcionalidad
3. No proporciona beneficios claros sobre lo que ya tenemos

Nuestra implementacion de x402 v2 ya esta funcionando. Las mejoras del upstream que realmente valen la pena pueden incorporarse via cherry-pick selectivo sin arriesgar nuestras customizaciones.

**Accion inmediata**: Comenzar con los cherry-picks de alta prioridad (BlobGasFiller, SPL no-entrypoint, Solana mint verification) y evaluar resultados antes de continuar.
