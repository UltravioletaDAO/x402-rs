# 10. Feature Flags / Compilacion Condicional

## Resumen

Upstream ha reestructurado completamente su binario facilitador usando **Cargo feature flags** para que cada familia de cadenas (EVM, Solana, Aptos) sea una dependencia opcional compilada condicionalmente. Tambien la telemetria (tracing/OpenTelemetry) es un feature flag separado. Nosotros ya usamos `#[cfg(feature = "...")]` para Algorand y Sui, pero el grueso de nuestro codigo (EVM, Solana, NEAR, Stellar) esta siempre compilado.

**Recomendacion: NO -- No adoptar el sistema de feature flags de upstream.**

Explicacion completa abajo.

---

## Implementacion Upstream

### Estructura del Workspace

Upstream reorganizo todo en crates independientes con features granulares:

```
crates/
  x402-types/             # Tipos base (con feature "telemetry", "cli")
  x402-facilitator-local/ # Logica de facilitador (con feature "telemetry")
  chains/
    x402-chain-eip155/    # EVM (con features: facilitator, client, server, telemetry)
    x402-chain-solana/    # Solana (con features: facilitator, client, server, telemetry)
    x402-chain-aptos/     # Aptos (con features: facilitator, telemetry)
facilitator/              # Binario que combina todo via features
```

### Cargo.toml del Facilitador Upstream

```toml
[features]
default = ["telemetry", "chain-eip155", "chain-solana"]
telemetry = [
    "dep:tracing",
    "x402-types/telemetry",
    "x402-facilitator-local/telemetry",
    "x402-chain-eip155?/telemetry",
    "x402-chain-solana?/telemetry",
    "x402-chain-aptos?/telemetry"
]
chain-aptos = ["dep:x402-chain-aptos"]
chain-eip155 = ["dep:x402-chain-eip155"]
chain-solana = ["dep:x402-chain-solana"]
full = ["telemetry", "chain-aptos", "chain-eip155", "chain-solana"]

[dependencies]
x402-chain-eip155 = { workspace = true, features = ["facilitator"], optional = true }
x402-chain-solana = { workspace = true, features = ["facilitator"], optional = true }
x402-chain-aptos = { workspace = true, features = ["facilitator"], optional = true }
```

Puntos clave:
- Cada cadena es `optional = true`
- El feature `telemetry` se propaga con `?` (solo si el crate de cadena esta habilitado)
- `default` incluye EVM + Solana + telemetria (Aptos es opt-in)

### Codigo Condicional en Upstream

**chain.rs** - El enum `ChainProvider` tiene variantes condicionales:

```rust
pub enum ChainProvider {
    #[cfg(feature = "chain-eip155")]
    Eip155(Arc<eip155::Eip155ChainProvider>),
    #[cfg(feature = "chain-solana")]
    Solana(Arc<solana::SolanaChainProvider>),
    #[cfg(feature = "chain-aptos")]
    Aptos(Arc<aptos::AptosChainProvider>),
}
```

**schemes.rs** - Registro de esquemas condicional:

```rust
#[cfg(feature = "chain-eip155")]
use x402_chain_eip155::{V1Eip155Exact, V2Eip155Exact};

#[cfg(feature = "chain-eip155")]
impl X402SchemeFacilitatorBuilder<&ChainProvider> for V2Eip155Exact {
    fn build(&self, provider: &ChainProvider, config: Option<serde_json::Value>)
        -> Result<Box<dyn X402SchemeFacilitator>, Box<dyn std::error::Error>> {
        let eip155_provider = if let ChainProvider::Eip155(provider) = provider {
            Arc::clone(provider)
        } else {
            return Err("provider must be an Eip155ChainProvider".into());
        };
        self.build(eip155_provider, config)
    }
}
```

**run.rs** - Registro dinamico de esquemas en startup:

```rust
let scheme_blueprints = {
    let mut scheme_blueprints = SchemeBlueprints::new();
    #[cfg(feature = "chain-eip155")]
    {
        scheme_blueprints.register(V1Eip155Exact);
        scheme_blueprints.register(V2Eip155Exact);
    }
    #[cfg(feature = "chain-solana")]
    {
        scheme_blueprints.register(V1SolanaExact);
        scheme_blueprints.register(V2SolanaExact);
    }
    #[cfg(feature = "chain-aptos")]
    {
        scheme_blueprints.register(V2AptosExact);
    }
    scheme_blueprints
};
```

**Telemetria condicional:**

```rust
#[cfg(feature = "telemetry")]
let telemetry_layer = {
    let telemetry = Telemetry::new()
        .with_name(env!("CARGO_PKG_NAME"))
        .with_version(env!("CARGO_PKG_VERSION"))
        .register();
    telemetry.http_tracing()
};

// ... mas adelante ...
#[cfg(feature = "telemetry")]
let http_endpoints = http_endpoints.layer(telemetry_layer);

#[cfg(feature = "telemetry")]
tracing::info!("Starting server at http://{}", addr);
```

---

## Nuestro Estado Actual

### Features Definidos en Cargo.toml

```toml
[features]
telemetry = []
solana = ["x402-compliance/solana"]
near = []
stellar = []
algorand = ["algonaut", "rmp-serde"]
sui = ["sui-sdk", "sui-types", "sui-keys", "shared-crypto", "bcs"]
```

### Uso Real de `#[cfg(feature)]` en Nuestro Codigo

| Archivo | `#[cfg(feature)]` | Features usados |
|---------|-------------------|-----------------|
| `src/network.rs` | 39 anotaciones | `algorand`, `sui` |
| `src/chain/mod.rs` | 18 anotaciones | `algorand`, `sui` |
| `src/types.rs` | 15 anotaciones | `algorand`, `sui` |
| `src/chain/evm.rs` | 10 anotaciones | `algorand`, `sui` |
| `src/from_env.rs` | 9 anotaciones | `algorand`, `sui` |
| `src/chain/solana.rs` | 7 anotaciones | `algorand`, `sui` |
| `src/caip2.rs` | 6 anotaciones | `sui` |
| `src/facilitator_local.rs` | 6 anotaciones | `solana`, `near`, `stellar`, `algorand`, `sui` |
| `src/handlers.rs` | 2 anotaciones | `algorand`, `sui` |
| **Total** | **112 anotaciones** | --- |

### Lo Que Esta Detras de Feature Flags (nosotros)

| Feature | Dependencias Opcionales | Impacto |
|---------|------------------------|---------|
| `sui` | `sui-sdk`, `sui-types`, `sui-keys`, `shared-crypto`, `bcs` | Sui SDK completo (git deps de MystenLabs) |
| `algorand` | `algonaut`, `rmp-serde` | SDK de Algorand |
| `solana` | (solo pasa a x402-compliance) | Solo afecta compliance, NO el chain provider |
| `near` | (vacio) | Flag informativo, no excluye dependencias |
| `stellar` | (vacio) | Flag informativo, no excluye dependencias |
| `telemetry` | (vacio) | Flag informativo, no excluye dependencias |

### Lo Que NO Esta Detras de Feature Flags (siempre compilado)

- **EVM completo**: `alloy` (64MB+ de dependencias), todos los chain providers EVM
- **Solana completo**: `solana-sdk`, `solana-client`, `spl-token`, `spl-token-2022`
- **NEAR completo**: `near-jsonrpc-client`, `near-primitives`, `near-crypto`
- **Stellar completo**: `stellar-xdr`, `stellar-strkey`, `reqwest`
- **Telemetria completa**: `tracing`, `opentelemetry`, `tracing-opentelemetry`, todos los exportadores OTLP
- **AWS SDK**: `aws-config`, `aws-sdk-s3`, `aws-sdk-dynamodb`
- **Swagger/OpenAPI**: `utoipa`, `utoipa-swagger-ui`

### Dockerfile Actual

```dockerfile
RUN cargo build --release --features solana,near,stellar,algorand,sui
```

Compila TODO -- los features `near` y `stellar` estan vacios (no excluyen nada).

### Tamano del Binario Actual

```
63MB  target/release/x402-rs
```

---

## Mapa Completo de Feature Flags (si los implementaramos)

### Propuesta Teorica

```toml
[features]
default = ["chain-evm", "chain-solana", "telemetry"]

# Familias de cadenas
chain-evm = ["dep:alloy"]
chain-solana = ["dep:solana-sdk", "dep:solana-client", "dep:spl-token", "dep:spl-token-2022"]
chain-near = ["dep:near-jsonrpc-client", "dep:near-primitives", "dep:near-crypto", ...]
chain-stellar = ["dep:stellar-xdr", "dep:stellar-strkey"]
chain-algorand = ["dep:algonaut", "dep:rmp-serde"]
chain-sui = ["dep:sui-sdk", "dep:sui-types", "dep:sui-keys", "dep:shared-crypto", "dep:bcs"]

# Telemetria
telemetry = ["dep:opentelemetry", "dep:opentelemetry_sdk", "dep:tracing-opentelemetry", "dep:opentelemetry-otlp"]

# Extras
swagger = ["dep:utoipa", "dep:utoipa-swagger-ui"]
aws = ["dep:aws-config", "dep:aws-sdk-s3", "dep:aws-sdk-dynamodb"]

# Todo
full = ["chain-evm", "chain-solana", "chain-near", "chain-stellar", "chain-algorand", "chain-sui", "telemetry", "swagger", "aws"]
```

### Archivos Que Necesitarian Cambios

| Archivo | Lineas | Cambios Necesarios |
|---------|--------|-------------------|
| `Cargo.toml` | 113 | Hacer 20+ dependencias opcionales, definir 10+ features |
| `src/network.rs` | 2206 | Agregar ~60 anotaciones `#[cfg]` para EVM, Solana, NEAR, Stellar, Fogo |
| `src/types.rs` | 2117 | Agregar ~30 anotaciones `#[cfg]` para NetworkFamily, Scheme variants |
| `src/chain/mod.rs` | 200 | Agregar `#[cfg]` para EVM, Solana, NEAR, Stellar (ya tiene Algorand/Sui) |
| `src/chain/evm.rs` | 1995 | Envolver todo en `#[cfg(feature = "chain-evm")]` o hacer el modulo condicional |
| `src/chain/solana.rs` | 1158 | Envolver todo en `#[cfg(feature = "chain-solana")]` |
| `src/chain/near.rs` | 733 | Envolver todo en `#[cfg(feature = "chain-near")]` |
| `src/chain/stellar.rs` | 1585 | Envolver todo en `#[cfg(feature = "chain-stellar")]` |
| `src/from_env.rs` | 492 | Agregar `#[cfg]` para constantes y funciones por cadena |
| `src/facilitator_local.rs` | 473 | Agregar `#[cfg]` en match arms de verify/settle/supported |
| `src/handlers.rs` | ~1500 | Agregar `#[cfg]` para rutas de logos por cadena |
| `src/caip2.rs` | ~250 | Agregar `#[cfg]` para parseo CAIP-2 por familia |
| `src/provider_cache.rs` | ~200 | Agregar `#[cfg]` para inicializacion de providers |
| `src/openapi.rs` | ~300 | Agregar `#[cfg]` para documentacion de endpoints por cadena |
| **Total estimado** | | **200+ anotaciones `#[cfg]` nuevas** |

---

## Analisis de Tamano del Binario

### Estimacion por Familia de Cadena

| Componente | Dependencias Principales | Tamano Estimado (contribucion al binario) |
|-----------|-------------------------|------------------------------------------|
| EVM (alloy) | alloy, alloy-provider, alloy-sol-types | ~15-20 MB |
| Solana | solana-sdk, solana-client, spl-token | ~12-18 MB |
| NEAR | near-jsonrpc-client, near-primitives, near-crypto | ~5-8 MB |
| Stellar | stellar-xdr, stellar-strkey | ~3-5 MB |
| Algorand | algonaut | ~2-3 MB |
| Sui | sui-sdk (git dep completo) | ~8-12 MB |
| Telemetria | opentelemetry, tracing-opentelemetry, otlp | ~3-5 MB |
| AWS SDK | aws-config, aws-sdk-s3, aws-sdk-dynamodb | ~4-6 MB |
| Swagger | utoipa, utoipa-swagger-ui | ~1-2 MB |
| Codigo propio + axum + tokio | Core siempre incluido | ~5-8 MB |

**Binario actual**: 63 MB (con todo compilado)

**Binario hipotetico solo-EVM**: ~25-30 MB (ahorro ~50%)
**Binario hipotetico solo-Solana**: ~20-25 MB
**Binario hipotetico sin telemetria**: 60 MB (ahorro ~5%)

### Impacto en Docker Image

La imagen Docker final usa `debian:bullseye-slim` (~80 MB) + binario (63 MB) + assets. Si el binario bajara a 30 MB, la imagen Docker bajaria de ~150 MB a ~115 MB. No es un cambio significativo para ECS.

---

## Impacto en Tiempo de Compilacion

### Build Actual (Todo Compilado)

- **Clean build**: ~3 min (en ~/x402-rs-build via fast-build.sh)
- **Incremental**: ~35s

### Build Hipotetico Solo-EVM

- **Clean build estimado**: ~1.5-2 min (eliminando Solana SDK que es pesado)
- **Incremental estimado**: ~25s

### Costo de la Refactorizacion

- La refactorizacion misma requiere multiples clean builds para verificar que cada combinacion de features compila
- Combinaciones a probar: `{}`, `{evm}`, `{solana}`, `{near}`, `{stellar}`, `{algorand}`, `{sui}`, `{evm,solana}`, `{full}`, etc.
- Con 6 familias de cadenas, hay 64 combinaciones posibles. En practica se prueban ~10-15 combinaciones criticas
- **Tiempo solo para verificar la refactorizacion**: 15-30 min de builds

---

## Pros y Contras

### Pros de Adoptar Feature Flags Completos

1. **Alineacion con upstream**: Facilita futuras sincronizaciones
2. **Binario mas pequeno**: Si solo desplegamos EVM+Solana, ahorramos ~20 MB
3. **Compilacion mas rapida**: Builds incrementales ligeramente mas rapidos
4. **Separacion limpia**: Fuerza buenas practicas de modularidad
5. **Despliegue selectivo**: Podriamos tener binarios especializados por cadena

### Contras de Adoptar Feature Flags Completos

1. **Divergencia masiva**: 200+ lineas de `#[cfg]` nuevas. Cada merge de upstream se complica enormemente
2. **Complejidad de mantenimiento**: Cada nuevo network requiere decidir su feature y propagarlo por 10+ archivos
3. **Nuestra arquitectura es diferente**: Upstream tiene crates separados por cadena (`x402-chain-eip155`, etc.). Nosotros tenemos todo en `src/chain/`. Los `#[cfg]` de upstream son limpios porque viven en la frontera entre crates. Los nuestros serian intrusivos dentro de un solo crate
4. **No necesitamos binarios parciales**: Siempre desplegamos UN binario con TODAS las cadenas. No hay caso de uso real para un binario solo-EVM
5. **Riesgo de bugs**: `#[cfg]` oculta codigo del compilador. Un typo en un feature flag no da error -- simplemente excluye el codigo silenciosamente
6. **Test matrix explosion**: Necesitariamos CI para multiples combinaciones de features. Actualmente no tenemos CI automatizado
7. **Telemetria siempre activa**: Nosotros siempre queremos tracing -- no hay razon para hacerlo opcional
8. **Los features vacios son inofensivos**: `near = []` y `stellar = []` no agregan overhead. Solo documentan intencion
9. **Costo real: 2-3 dias de trabajo** con alto riesgo de regresiones

### Por Que Upstream Lo Hizo (y por que no aplica a nosotros)

Upstream es una **libreria publica** publicada en crates.io. Sus usuarios (desarrolladores que importan `x402-chain-eip155` como dependencia) necesitan controlar que dependencias transitivas se incluyen en sus proyectos. Un servidor web que solo usa EVM no quiere compilar el SDK de Aptos.

Nosotros somos un **servicio desplegable**. No publicamos crates. No tenemos consumidores externos. El binario final siempre incluye todo.

---

## Estimacion de Esfuerzo

| Tarea | Horas |
|-------|-------|
| Hacer dependencias EVM opcionales + agregar `#[cfg]` a evm.rs, network.rs, types.rs | 4-6h |
| Hacer dependencias Solana opcionales + agregar `#[cfg]` | 3-4h |
| Hacer dependencias NEAR opcionales + agregar `#[cfg]` | 2-3h |
| Hacer dependencias Stellar opcionales + agregar `#[cfg]` | 2-3h |
| Hacer telemetria opcional + agregar `#[cfg]` | 2-3h |
| Actualizar Dockerfile, scripts de build, CI | 1-2h |
| Testing de todas las combinaciones de features | 3-4h |
| Debugging de errores de compilacion por features faltantes | 2-4h |
| **Total** | **19-29 horas (2-3 dias)** |

---

## Que SI Podemos Hacer (Mejoras Incrementales)

En lugar de la refactorizacion completa, mantener nuestro sistema actual con mejoras puntuales:

### 1. Mantener Algorand y Sui como features (ya esta hecho)
Esto funciona bien porque ambos tienen dependencias pesadas y opcionales (git deps de Sui, algonaut).

### 2. Agregar feature flag para NEAR si sus deps se vuelven problematicas
Actualmente NEAR compila rapido y no pesa mucho. Si las dependencias de NEAR crecen, podemos hacerlo opcional.

### 3. NO hacer EVM o Solana opcionales
Son el 95% de nuestro trafico. Siempre estaran habilitados. Hacerlos opcionales solo agrega complejidad.

### 4. Mantener telemetria siempre habilitada
No hay escenario donde queramos un binario sin tracing en produccion.

### 5. Para la proxima sync de upstream
Cuando sincronicemos con upstream, NO intentar adoptar su sistema de features. En lugar de eso, cherry-pick los cambios de logica de negocio y ignorar la reestructuracion de crates/features.

---

## Recomendacion Final

**NO. No adoptar el sistema de feature flags de upstream.**

**Justificacion en una frase**: Somos un servicio desplegable, no una libreria publicable. Siempre compilamos y desplegamos todo. El costo de refactorizacion (2-3 dias + riesgo de regresiones + complejidad de merges futuros) no se justifica para un ahorro de ~20 MB en binario y ~30s en builds.

**Lo que SI hacer**:
- Mantener los feature flags existentes (`sui`, `algorand`) que protegen dependencias pesadas y opcionales
- Seguir usando `--features solana,near,stellar,algorand,sui` en Dockerfile
- En la proxima sync, cherry-pick logica de negocio de upstream sin adoptar su reestructuracion de crates
- Si en el futuro necesitamos publicar crates (por ejemplo, `x402-axum` o `x402-reqwest`), ENTONCES reconsiderar la modularizacion

**Prioridad**: Baja. No hay urgencia ni beneficio tangible.
