# 09 - Refactoring Modular del Workspace

**Fecha**: 2026-02-12
**Estado**: ANALISIS COMPLETO
**Impacto**: Arquitectura fundamental del proyecto
**Esfuerzo estimado**: 15-25 persona-dias

---

## Resumen Ejecutivo

Upstream reorganizo su monolito en 7+ crates publicados en crates.io, adoptando una arquitectura modular con separacion clara entre tipos, facilitador, y cadenas especificas. Nuestro fork mantiene un monolito de ~28,400 lineas en `src/` con 6+ cadenas adicionales y modulos propios sin equivalente upstream (escrow, discovery, ERC-8004, payment_operator, blocklist, FHE proxy).

**Recomendacion: NO migrar a la estructura upstream. Adopcion parcial selectiva de x402-types como dependencia.**

---

## Arquitectura Upstream (v1.1.3)

### Workspace: 7 crates + 1 binario

```
x402-rs/                          (workspace root)
|
+-- crates/
|   +-- x402-types/               Tipos core, CAIP-2, protocolo v1/v2, traits
|   +-- x402-facilitator-local/   FacilitatorLocal + handlers HTTP + telemetria
|   +-- x402-axum/                Middleware Axum para servidores protegidos
|   +-- x402-reqwest/             Cliente reqwest con soporte x402
|   +-- chains/
|       +-- x402-chain-eip155/    Cadenas EVM (EIP-3009, ERC-6492)
|       +-- x402-chain-solana/    Cadenas Solana
|       +-- x402-chain-aptos/     Cadenas Aptos (no publicable)
|
+-- facilitator/                  Binario servidor (solo ensamblaje)
+-- examples/
+-- patches/                      Parches para dependencias Aptos
```

### Grafo de Dependencias

```
x402-types                    (base, sin dependencias internas)
    ^           ^         ^
    |           |         |
x402-chain-   x402-chain-  x402-chain-
eip155        solana        aptos
    ^           ^         ^
    |           |         |
    +-----+-----+---------+
          |
  x402-facilitator-local  (handlers + FacilitatorLocal)
          ^
          |
  facilitator/            (binario: config JSON + ensamblaje)
```

### Caracteristicas Clave de la Arquitectura Upstream

1. **Configuracion via JSON** (`config.json`), no variables de entorno directas
2. **LiteralOrEnv<T>** permite referenciar env vars desde JSON (`"$API_KEY"`)
3. **SchemeRegistry** patron plugin: cada cadena registra sus esquemas (v1-eip155-exact, v2-solana-exact, etc.)
4. **ChainRegistry** con `FromConfig` trait para inicializar providers desde JSON
5. **Feature flags** granulares: `chain-eip155`, `chain-solana`, `chain-aptos`, `telemetry`
6. **Rust edition 2024**, requiere Rust 1.88+
7. **Solo 3 familias de cadenas**: EVM, Solana, Aptos
8. **Sin landing page custom**, sin escrow, sin discovery, sin blocklist, sin ERC-8004

### Archivos por Crate (upstream)

| Crate | Archivos .rs | Lineas aprox. | Publicado |
|-------|-------------|---------------|-----------|
| x402-types | 17 | ~3,500 | Si (crates.io) |
| x402-chain-eip155 | 12 | ~4,000 | Si |
| x402-chain-solana | 12 | ~3,500 | Si |
| x402-chain-aptos | 7 | ~1,500 | No (dep git) |
| x402-facilitator-local | 6 | ~2,000 | Si |
| x402-axum | 4 | ~800 | Si |
| x402-reqwest | 3 | ~500 | Si |
| facilitator (bin) | 5 | ~700 | No |
| **Total** | **~66** | **~16,500** | - |

---

## Nuestra Arquitectura Actual (v1.33.3)

### Monolito con workspace minimo

```
x402-rs/                          (crate principal, binario + libreria)
|
+-- src/
|   +-- main.rs                   Servidor HTTP (317 lineas)
|   +-- handlers.rs               Handlers HTTP + landing page (3,381 lineas)
|   +-- network.rs                Definiciones de redes (2,206 lineas)
|   +-- types.rs                  Tipos protocolo v1 (2,117 lineas)
|   +-- types_v2.rs               Tipos protocolo v2 (1,476 lineas)
|   +-- facilitator.rs            Trait Facilitator (113 lineas)
|   +-- facilitator_local.rs      Implementacion local (473 lineas)
|   +-- from_env.rs               Config via env vars (492 lineas)
|   +-- provider_cache.rs         Cache de providers RPC (115 lineas)
|   +-- caip2.rs                  Parsing CAIP-2 (489 lineas)
|   +-- escrow.rs                 Escrow lifecycle (970 lineas)
|   +-- discovery.rs              Bazaar discovery (923 lineas)
|   +-- discovery_aggregator.rs   Agregador discovery (1,066 lineas)
|   +-- discovery_crawler.rs      Crawler discovery (482 lineas)
|   +-- discovery_store.rs        Store DynamoDB (468 lineas)
|   +-- nonce_store.rs            Store DynamoDB nonces (458 lineas)
|   +-- blocklist.rs              Lista negra OFAC (204 lineas)
|   +-- fhe_proxy.rs              Proxy FHE (205 lineas)
|   +-- openapi.rs                Swagger/OpenAPI (900 lineas)
|   +-- telemetry.rs              OpenTelemetry (397 lineas)
|   +-- sig_down.rs               Signal handling (53 lineas)
|   +-- debug_utils.rs            Utilidades debug (123 lineas)
|   +-- timestamp.rs              Timestamps EIP-3009 (68 lineas)
|   +-- lib.rs                    Reexports (47 lineas)
|   +-- chain/
|   |   +-- evm.rs                EVM settlement (1,995 lineas)
|   |   +-- solana.rs             Solana settlement (1,158 lineas)
|   |   +-- stellar.rs            Stellar settlement (1,585 lineas)
|   |   +-- near.rs               NEAR settlement (733 lineas)
|   |   +-- sui.rs                Sui settlement (609 lineas)
|   |   +-- algorand.rs           Algorand settlement (954 lineas)
|   |   +-- mod.rs                Chain dispatch (200 lineas)
|   +-- erc8004/                  ERC-8004 on-chain registries (1,539 lineas)
|   +-- payment_operator/         PaymentOperator EM (2,095 lineas)
|
+-- crates/
|   +-- x402-axum/                Middleware Axum (nuestro, divergido)
|   +-- x402-reqwest/             Cliente reqwest (nuestro, divergido)
|   +-- x402-compliance/          Compliance/blocklist (propio)
```

### Metricas Comparativas

| Metrica | Upstream | Nosotros | Delta |
|---------|----------|----------|-------|
| Lineas .rs totales | ~16,500 | ~28,400 | +72% |
| Crates en workspace | 7 + bin | 3 + root | -57% |
| Cadenas soportadas | 3 (EVM, Solana, Aptos) | 7 (EVM, Solana, Stellar, NEAR, Sui, Algorand + parcial Aptos) | +133% |
| Redes EVM | ~14 | ~20+ | +43% |
| Stablecoins | Solo USDC | USDC + EURC + AUSD + mas | Significativo |
| Configuracion | JSON file | Variables de entorno + AWS SM | Incompatible |
| Rust edition | 2024 (1.88+) | 2021 (1.82+) | Incompatible |
| Features custom | 0 | escrow, discovery, ERC-8004, payment_operator, blocklist, FHE, OpenAPI | 7+ modulos |

---

## Mapeo Modulo-a-Crate

### Modulos con equivalente upstream directo

| Nuestro modulo | Crate upstream | Divergencia | Migrabilidad |
|----------------|---------------|-------------|--------------|
| `src/types.rs` | `x402-types/src/proto/v1.rs` | Alta (nuestros tipos incluyen campos custom) | Baja |
| `src/types_v2.rs` | `x402-types/src/proto/v2.rs` | Media (CAIP-2 similar) | Media |
| `src/caip2.rs` | `x402-types/src/chain/chain_id.rs` | Media (mismo concepto, API distinta) | Media |
| `src/facilitator.rs` | `x402-types/src/facilitator.rs` | Baja (mismo trait, nosotros +blacklist_info) | Alta |
| `src/facilitator_local.rs` | `x402-facilitator-local/` | Alta (nosotros dispatch manual, ellos SchemeRegistry) | Baja |
| `src/timestamp.rs` | `x402-types/src/timestamp.rs` | Baja | Alta |
| `src/chain/evm.rs` | `x402-chain-eip155/` | Muy alta (2,000 vs 4,000 lineas, arquitectura distinta) | Muy baja |
| `src/chain/solana.rs` | `x402-chain-solana/` | Alta (versiones SDK distintas) | Baja |
| `src/sig_down.rs` | `x402-facilitator-local/src/util/sig_down.rs` | Minima | Alta |
| `src/network.rs` | `x402-types/src/networks.rs` + chain crate `networks.rs` | Extrema (nuestro es 2,206 lineas con 20+ redes y tokens custom) | Nula |
| `src/from_env.rs` | `x402-types/src/config.rs` + `facilitator/src/config.rs` | Total (env vars vs JSON file) | Nula |
| `src/handlers.rs` | `x402-facilitator-local/src/handlers.rs` | Extrema (3,381 vs ~300 lineas, landing page, logos, OpenAPI, escrow, discovery) | Nula |
| `src/telemetry.rs` | `x402-facilitator-local/src/util/telemetry.rs` | Media (mismos conceptos, versiones OTel distintas) | Media |

### Modulos SIN equivalente upstream (100% custom)

| Modulo | Lineas | Proposito |
|--------|--------|-----------|
| `src/escrow.rs` | 970 | Ciclo de vida escrow gasless |
| `src/discovery.rs` | 923 | Bazaar discovery protocol |
| `src/discovery_aggregator.rs` | 1,066 | Agregacion multi-facilitador |
| `src/discovery_crawler.rs` | 482 | Crawler de descubrimiento |
| `src/discovery_store.rs` | 468 | Persistencia DynamoDB discovery |
| `src/nonce_store.rs` | 458 | Persistencia DynamoDB nonces |
| `src/blocklist.rs` | 204 | Lista negra OFAC/compliance |
| `src/fhe_proxy.rs` | 205 | Proxy cifrado homomorfico |
| `src/openapi.rs` | 900 | Swagger UI + OpenAPI spec |
| `src/erc8004/` (3 archivos) | 1,539 | Registros on-chain ERC-8004 |
| `src/payment_operator/` (6 archivos) | 2,095 | PaymentOperator EM |
| `src/chain/stellar.rs` | 1,585 | Stellar/Soroban settlement |
| `src/chain/near.rs` | 733 | NEAR NEP-366 meta-txns |
| `src/chain/sui.rs` | 609 | Sui sponsored txns |
| `src/chain/algorand.rs` | 954 | Algorand atomic groups |
| `src/debug_utils.rs` | 123 | Utilidades diagnostico |
| **Total custom** | **~13,314** | **47% de nuestro codigo** |

---

## Analisis de Migracion

### Opcion A: Migracion Completa (alinear con estructura upstream)

**Que implicaria:**
1. Extraer tipos a un crate `x402-types` compatible con upstream
2. Extraer `chain/evm.rs` a `x402-chain-eip155` compatible
3. Extraer `chain/solana.rs` a `x402-chain-solana` compatible
4. Crear crates nuevos para cadenas sin upstream: `x402-chain-stellar`, `x402-chain-near`, `x402-chain-sui`, `x402-chain-algorand`
5. Migrar configuracion de env vars a JSON (rompe todo el deployment actual)
6. Migrar de Rust edition 2021 a 2024 (requiere Rust 1.88+)
7. Adaptar nuestro `SchemeRegistry` pattern vs el dispatch manual actual
8. Decidir donde viven escrow, discovery, ERC-8004, payment_operator, blocklist

**Esfuerzo: 25-35 persona-dias**

**Riesgo: CRITICO** - Reescritura casi total del proyecto. Production downtime probable.

### Opcion B: Adopcion Parcial (consumir x402-types de crates.io)

**Que implicaria:**
1. Agregar `x402-types = "1.0"` como dependencia
2. Migrar progresivamente nuestros `types.rs` y `types_v2.rs` para usar los tipos upstream
3. Adoptar `ChainId` de upstream para CAIP-2 (reemplaza nuestro `caip2.rs`)
4. Mantener todo lo demas como esta (handlers, network, chains, config, features custom)

**Esfuerzo: 5-8 persona-dias**

**Riesgo: BAJO** - Cambio incremental, compatible con deployment actual

### Opcion C: No Migrar (mantener monolito, cherry-pick selectivo)

**Que implicaria:**
1. Seguir con la arquitectura actual
2. Cherry-pick features especificos de upstream (pending nonce manager, etc.)
3. Adaptar manualmente los cambios relevantes

**Esfuerzo: 0 persona-dias (base) + 1-2 dias por feature cherry-picked**

**Riesgo: MINIMO** - Sin cambios arquitectonicos

---

## Pros y Contras

### Pros de Migrar (Opcion A o B)

1. **Merges upstream mas faciles** - Si la estructura coincide, `git merge` funciona mejor
2. **Tipos compartidos** - Consumir `x402-types` de crates.io elimina drift en protocolo
3. **Compilacion incremental** - Crates separados permiten recompilar solo lo cambiado
4. **Reusabilidad** - Otros proyectos podrian consumir nuestros crates de cadena
5. **Testing aislado** - Cada crate con sus propios tests unitarios

### Contras de Migrar

1. **47% de nuestro codigo no tiene equivalente upstream** - Los 13,314 lineas de modulos custom (escrow, discovery, ERC-8004, payment_operator, 4 cadenas extra) no encajan en ninguna estructura upstream
2. **Incompatibilidad de configuracion** - Upstream usa JSON, nosotros env vars + AWS Secrets Manager. Migrar rompe toda la infraestructura de deployment (Terraform, ECS task definitions, secrets rotation)
3. **Incompatibilidad de Rust edition** - 2021 vs 2024. Nuestras dependencias (solana-sdk 2.x, sui-sdk git, algonaut) pueden no compilar en edition 2024
4. **Divergencia de tipos** - Nuestros `types.rs` incluyen campos adicionales (escrow, compliance, domain hints) que no existen en x402-types upstream
5. **4 cadenas extra sin crate upstream** - Stellar, NEAR, Sui, Algorand necesitarian crates nuevos que solo nosotros mantenemos. No hay beneficio de "alinear con upstream" si upstream no tiene esas cadenas
6. **Riesgo operacional** - El facilitador esta en produccion procesando pagos reales. Una reestructuracion masiva introduce riesgo de regresion en settlement (perdida de fondos potencial)
7. **Merges seran raros de todas formas** - La divergencia ya es tan grande que ningun merge sera automatico. La estructura de archivos es lo de menos; la divergencia semantica es el problema real
8. **Configuracion JSON vs env vars es un downgrade para nuestro caso** - AWS ECS con Secrets Manager funciona mucho mejor con env vars. JSON files requeririan montar secrets como archivos, mas complejo

### Sobre la Compilacion Incremental

Este argumento merece atencion especial. En teoria, crates separados permiten `cargo build` incremental mas rapido. En la practica:

- Ya usamos `fast-build.sh` con rsync a ext4 nativo (35 segundos incrementales)
- La mayor parte del tiempo de compilacion es linking, no compilacion
- Nuestras dependencias pesadas (solana-sdk, sui-sdk, alloy) no se benefician de la separacion
- En CI (GitHub Actions), cada build es limpio de todas formas

**El beneficio real de compilacion incremental es marginal para nuestro caso.**

---

## Recomendacion

### NO migrar a la estructura modular upstream

**Razonamiento:**

1. **La divergencia es irreconciliable** - Con 47% de codigo custom, 4 cadenas extra, sistema de configuracion completamente distinto, y modulos enteros sin equivalente upstream, alinear la estructura de archivos no resuelve el problema real de sincronizacion.

2. **El costo no justifica el beneficio** - 25-35 persona-dias para una reestructuracion con riesgo alto de regresion, a cambio de merges upstream que de todas formas requieren revision manual linea por linea.

3. **La configuracion JSON es un retroceso para produccion en ECS** - Nuestro sistema de env vars + AWS Secrets Manager es superior para el entorno de deployment actual. Migrar a JSON agregaria complejidad sin beneficio.

4. **Rust edition 2024 no es factible hoy** - Las dependencias de cadenas no-EVM (solana-sdk 2.x, sui-sdk, algonaut, near-primitives) pueden tener problemas con edition 2024 / Rust 1.88+.

### SI considerar: Adopcion parcial de x402-types (Opcion B, diferida)

Cuando las siguientes condiciones se cumplan:
- x402-types alcance version 2.0+ con API estable
- Nuestros tipos custom (escrow fields, compliance fields) puedan expresarse via extension sin fork
- Haya un caso de negocio para interoperabilidad de tipos con terceros

**Hasta entonces, la Opcion C (cherry-pick selectivo) es la ruta correcta.**

---

## Plan de Adopcion Parcial (si se decide en el futuro)

### Fase 1: x402-types como dependencia (5 dias)

```toml
# Cargo.toml
[dependencies]
x402-types = "1.0"  # Tipos upstream
```

1. Reemplazar `src/caip2.rs` con `x402_types::chain::ChainId` (1 dia)
2. Crear wrappers `From<OurType> for x402_types::proto::VerifyRequest` y viceversa (2 dias)
3. Adaptar `src/facilitator.rs` trait para aceptar tipos upstream en la interfaz publica (1 dia)
4. Tests de regresion en todos los endpoints (1 dia)

### Fase 2: Alinear protocolo v2 (3 dias)

1. Migrar `src/types_v2.rs` para reexportar desde `x402-types` donde sea posible (2 dias)
2. Mantener nuestras extensiones como newtype wrappers (1 dia)

### Fase 3: Evaluar chain crates (depende de resultados Fase 1-2)

Solo si Fase 1-2 demuestra beneficio medible:
1. Evaluar si `x402-chain-eip155` puede reemplazar nuestro `chain/evm.rs`
2. Evaluar si `x402-chain-solana` puede reemplazar nuestro `chain/solana.rs`
3. Las 4 cadenas custom (Stellar, NEAR, Sui, Algorand) NUNCA se migrarian a crates upstream

---

## Que Vigilar (What to Watch)

Dado que la recomendacion es NO migrar ahora, estos son los eventos que deberian disparar una reevaluacion:

1. **Upstream publica x402-types 2.0 con breaking changes** - Si los tipos del protocolo cambian significativamente, podria ser mas facil consumir el crate que mantener una copia divergida
2. **Upstream agrega cadenas que nosotros ya tenemos** - Si upstream agrega soporte Stellar o NEAR, podriamos consumir sus crates de cadena en vez de mantener los nuestros
3. **Necesitamos publicar nuestros propios crates en crates.io** - Si terceros quieren consumir nuestros tipos o middleware, la modularizacion seria necesaria
4. **El monolito se vuelve inmanejable** - Si `src/` supera 40,000+ lineas, la compilacion local se volveria dolorosa
5. **Upstream migra a un esquema de configuracion mas flexible** - Si soportan env vars nativamente, una barrera clave desaparece
6. **Rust edition 2024 se vuelve necesaria** - Si una dependencia critica requiere edition 2024, habria que resolver todas las incompatibilidades de todas formas

### Metricas de Umbral

| Metrica | Valor actual | Umbral para reevaluar |
|---------|-------------|----------------------|
| Lineas en src/ | 28,400 | > 40,000 |
| Tiempo build incremental | ~35s | > 120s |
| Frecuencia merge upstream | Trimestral | > Mensual |
| Cadenas compartidas con upstream | 2 (EVM, Solana) | > 4 |
| Consumidores externos de nuestros tipos | 0 | > 2 |

---

## Resumen de Linea de Fondo

| Aspecto | Veredicto |
|---------|-----------|
| Migrar estructura completa | **NO** - Costo desproporcionado, riesgo alto, beneficio marginal |
| Adoptar x402-types crate | **DIFERIR** - Evaluar cuando alcance v2.0 estable |
| Adoptar x402-chain-eip155 | **NO** - Nuestra implementacion EVM tiene demasiadas customizaciones |
| Adoptar x402-chain-solana | **NO** - Versiones SDK incompatibles, arquitectura distinta |
| Crear chain crates propios | **NO** - Sin consumidores externos, complejidad innecesaria |
| Migrar config a JSON | **NO** - Retroceso para deployment AWS ECS |
| Migrar a Rust edition 2024 | **NO** - Dependencias no-EVM incompatibles |
| Cherry-pick features individuales | **SI** - Caso por caso, como pending nonce manager |
