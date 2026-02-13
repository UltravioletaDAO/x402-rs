# 01 - Adopcion de PendingNonceManager desde Upstream

**Fecha**: 2026-02-12
**Estado**: IMPLEMENTADO - Ya adoptado previamente
**Esfuerzo estimado**: 0 lineas de codigo (ya esta en produccion)
**Riesgo**: Ninguno
**Version actual del fork**: v1.32.1+
**Version upstream analizada**: v1.1.3 (rama `upstream/main`)

---

## Resumen Ejecutivo

**Resultado del analisis**: El `PendingNonceManager` de upstream YA ESTA IMPLEMENTADO en nuestro fork. La implementacion es funcionalmente identica a la de upstream, con diferencias menores de estilo. **No se requiere ninguna accion.**

Este documento detalla el analisis comparativo completo entre ambas implementaciones para dejar constancia y servir como referencia futura.

---

## 1. Resumen de la Feature

### Que hace

`PendingNonceManager` es un gestor de nonces personalizado para transacciones EVM que resuelve el problema de errores "nonce too low" cuando el facilitador se reinicia mientras hay transacciones pendientes en el mempool.

### Por que importa

El problema concreto que resuelve:

1. El facilitador envia una transaccion EVM (ej: `transferWithAuthorization` en Base mainnet)
2. La transaccion entra al mempool pero aun no se confirma
3. El facilitador se reinicia (deploy en ECS, crash, etc.)
4. Al reiniciar, el nonce manager de Alloy por defecto (`CachedNonceManager`) consulta el nonce **confirmado** on-chain
5. Ese nonce no incluye las transacciones pendientes en mempool
6. La siguiente transaccion usa el mismo nonce -> error "nonce too low"

La solucion: usar `.pending()` al consultar el nonce inicial, lo cual incluye transacciones en mempool.

### Componentes clave

- **Estructura `PendingNonceManager`**: Cache thread-safe de nonces por direccion usando `DashMap<Address, Arc<Mutex<u64>>>`
- **Trait `NonceManager`**: Implementacion del trait de Alloy para integrarse con el provider stack
- **Metodo `reset_nonce()`**: Invalida el cache para una direccion especifica tras errores de transaccion
- **Sentinel value `u64::MAX`**: Indica que el nonce necesita ser re-consultado al RPC

---

## 2. Estado Actual - Nuestro Fork

### Archivo: `/mnt/z/ultravioleta/dao/x402-rs/src/chain/evm.rs`

**El `PendingNonceManager` ya esta implementado en nuestro fork**, integrado directamente en el archivo `evm.rs`. La implementacion vive en las lineas 1752-1830, con tests unitarios en las lineas 1832-1994.

#### Definicion de la estructura (linea 1772-1776)

```rust
#[derive(Clone, Debug, Default)]
pub struct PendingNonceManager {
    /// Cache of nonces per address. Each address has its own mutex-protected nonce value.
    nonces: Arc<DashMap<alloy::primitives::Address, Arc<Mutex<u64>>>>,
}
```

#### Implementacion del trait NonceManager (lineas 1778-1814)

```rust
#[async_trait]
impl NonceManager for PendingNonceManager {
    async fn get_next_nonce<P, N>(
        &self,
        provider: &P,
        address: alloy::primitives::Address,
    ) -> alloy::transports::TransportResult<u64>
    where
        P: Provider<N>,
        N: alloy::network::Network,
    {
        const NONE: u64 = u64::MAX;

        let nonce = {
            let rm = self
                .nonces
                .entry(address)
                .or_insert_with(|| Arc::new(Mutex::new(NONE)));
            Arc::clone(rm.value())
        };

        let mut nonce = nonce.lock().await;
        let new_nonce = if *nonce == NONE {
            tracing::trace!(%address, "fetching nonce");
            provider.get_transaction_count(address).pending().await?
        } else {
            tracing::trace!(%address, current_nonce = *nonce, "incrementing nonce");
            *nonce + 1
        };
        *nonce = new_nonce;
        Ok(new_nonce)
    }
}
```

#### Metodo reset_nonce (lineas 1816-1830)

```rust
impl PendingNonceManager {
    pub async fn reset_nonce(&self, address: Address) {
        if let Some(nonce_lock) = self.nonces.get(&address) {
            let mut nonce = nonce_lock.lock().await;
            *nonce = u64::MAX; // NONE sentinel - will trigger fresh query
            tracing::debug!(%address, "reset nonce cache, will requery on next use");
        }
    }
}
```

#### Integracion en el type system del provider (lineas 86-98)

```rust
/// Combined filler type for gas, blob gas, nonce, and chain ID.
type InnerFiller = JoinFill<
    GasFiller,
    JoinFill<BlobGasFiller, JoinFill<NonceFiller<PendingNonceManager>, ChainIdFiller>>,
>;

/// The fully composed Ethereum provider type used in this project.
pub type InnerProvider = FillProvider<
    JoinFill<JoinFill<Identity, InnerFiller>, WalletFiller<EthereumWallet>>,
    RootProvider,
>;
```

#### Almacenamiento en EvmProvider (lineas 204-218)

```rust
pub struct EvmProvider {
    inner: InnerProvider,
    eip1559: bool,
    chain: EvmChain,
    signer_addresses: Arc<Vec<Address>>,
    signer_cursor: Arc<AtomicUsize>,
    nonce_manager: PendingNonceManager,  // <-- campo dedicado
}
```

#### Construccion en try_new (lineas 222-272)

```rust
pub async fn try_new(
    wallet: EthereumWallet,
    rpc_url: &str,
    eip1559: bool,
    network: Network,
) -> Result<Self, Box<dyn std::error::Error>> {
    // ...
    let nonce_manager = PendingNonceManager::default();

    let filler = JoinFill::new(
        GasFiller,
        JoinFill::new(
            BlobGasFiller,
            JoinFill::new(
                NonceFiller::new(nonce_manager.clone()),
                ChainIdFiller::default(),
            ),
        ),
    );

    let inner = ProviderBuilder::default()
        .filler(filler)
        .wallet(wallet)
        .connect_client(client);

    Ok(Self {
        inner,
        eip1559,
        chain,
        signer_addresses,
        signer_cursor,
        nonce_manager,  // <-- se guarda referencia para reset_nonce()
    })
}
```

#### Uso de reset_nonce en send_transaction (lineas 365-484)

El `reset_nonce` se invoca en dos escenarios dentro de `send_transaction()`:

**Escenario 1 - Fallo al obtener receipt (linea 429)**:
```rust
Err(e) => {
    // Receipt fetch failed (timeout or other) - reset nonce
    // Do NOT retry: TX may have been mined, retrying could double-spend
    self.nonce_manager.reset_nonce(from_address).await;
    Err(FacilitatorLocalError::ContractCall(format!("{e:?}")))
}
```

**Escenario 2 - Error al enviar transaccion (linea 436)**:
```rust
Err(e) => {
    let error_str = format!("{e:?}");
    self.nonce_manager.reset_nonce(from_address).await;
    // ... nonce retry logic with safety checks
}
```

Ademas, nuestro fork incluye logica adicional de retry con proteccion anti-replay que upstream NO tiene:

- `MAX_NONCE_RETRIES = 1` (un reintento maximo tras error de nonce)
- Snapshot de `pre_send_nonce` antes de enviar
- Verificacion de `post_nonce > pre_send_nonce` para detectar si la TX original fue minada por otro nodo RPC
- Backoff de 250ms antes del reintento
- Funcion helper `is_nonce_error()` para clasificar errores

#### Tests unitarios (lineas 1832-1994)

Nuestro fork incluye 6 tests unitarios completos:

| Test | Linea | Descripcion |
|------|-------|-------------|
| `test_is_nonce_error` | 1838 | Clasifica correctamente errores de nonce |
| `test_reset_nonce_clears_cache` | 1853 | Reset cambia nonce a sentinel u64::MAX |
| `test_reset_nonce_after_allocation_sequence` | 1886 | Reset funciona tras multiples asignaciones |
| `test_reset_nonce_on_nonexistent_address` | 1914 | Reset no falla en direccion no existente |
| `test_multiple_addresses_independent_nonces` | 1926 | Nonces independientes por direccion |
| `test_concurrent_reset_and_access` | 1960 | Thread-safety en resets concurrentes |

---

## 3. Implementacion Upstream

### Archivo: `crates/chains/x402-chain-eip155/src/chain/pending_nonce_manager.rs`

Upstream tiene el `PendingNonceManager` en un archivo separado dentro de su estructura de workspace multi-crate.

#### Codigo completo de upstream

```rust
//! Nonce management for concurrent EVM transaction submission.

use alloy_primitives::Address;
use alloy_provider::Provider;
use alloy_provider::fillers::NonceManager;
use alloy_transport::TransportResult;
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Debug, Default)]
pub struct PendingNonceManager {
    nonces: Arc<DashMap<Address, Arc<Mutex<u64>>>>,
}

#[async_trait]
impl NonceManager for PendingNonceManager {
    async fn get_next_nonce<P, N>(&self, provider: &P, address: Address) -> TransportResult<u64>
    where
        P: Provider<N>,
        N: alloy_network::Network,
    {
        const NONE: u64 = u64::MAX;

        let nonce = {
            let rm = self
                .nonces
                .entry(address)
                .or_insert_with(|| Arc::new(Mutex::new(NONE)));
            Arc::clone(rm.value())
        };

        let mut nonce = nonce.lock().await;
        let new_nonce = if *nonce == NONE {
            #[cfg(feature = "telemetry")]
            tracing::trace!(%address, "fetching nonce");
            provider.get_transaction_count(address).pending().await?
        } else {
            #[cfg(feature = "telemetry")]
            tracing::trace!(%address, current_nonce = *nonce, "incrementing nonce");
            *nonce + 1
        };
        *nonce = new_nonce;
        Ok(new_nonce)
    }
}

impl PendingNonceManager {
    pub async fn reset_nonce(&self, address: Address) {
        if let Some(nonce_lock) = self.nonces.get(&address) {
            let mut nonce = nonce_lock.lock().await;
            *nonce = u64::MAX;
            #[cfg(feature = "telemetry")]
            tracing::debug!(%address, "reset nonce cache, will requery on next use");
        }
    }
}
```

---

## 4. Comparacion Detallada: Fork vs Upstream

### Tabla de diferencias

| Aspecto | Nuestro Fork | Upstream |
|---------|-------------|----------|
| **Ubicacion** | `src/chain/evm.rs` (lineas 1752-1830) | `src/chain/pending_nonce_manager.rs` (archivo separado) |
| **Estructura** | Identica: `Arc<DashMap<Address, Arc<Mutex<u64>>>>` | Identica |
| **Algoritmo get_next_nonce** | Identico: sentinel `u64::MAX`, `.pending()` en primera consulta | Identico |
| **reset_nonce** | Identico: reset a `u64::MAX` sentinel | Identico |
| **Tracing** | Siempre habilitado (sin feature gate) | Detras de `#[cfg(feature = "telemetry")]` |
| **Imports de Alloy** | Via crate umbrella `alloy::*` | Via crates individuales `alloy_primitives`, `alloy_provider`, etc. |
| **Dependencia dashmap** | `dashmap = "6.1.0"` en Cargo.toml raiz | `dashmap = "6.1.0"` como dependencia opcional (feature `facilitator`) |
| **Tests** | 6 tests unitarios + `is_nonce_error` test | Sin tests en el archivo |
| **Retry logic** | Integrada en `send_transaction()` con anti-replay | No visible en el archivo del nonce manager |

### Diferencias en detalle

#### 1. Feature gate para tracing

**Upstream** envuelve los `tracing::trace!` y `tracing::debug!` con `#[cfg(feature = "telemetry")]`:
```rust
#[cfg(feature = "telemetry")]
tracing::trace!(%address, "fetching nonce");
```

**Nuestro fork** tiene tracing siempre habilitado:
```rust
tracing::trace!(%address, "fetching nonce");
```

**Evaluacion**: Nuestra decision es correcta para un servicio de produccion. El feature gate de upstream tiene sentido para una biblioteca reutilizable que puede ser usada sin tracing, pero nuestro facilitador siempre quiere trazabilidad.

#### 2. Imports de Alloy

**Upstream** usa crates individuales de Alloy:
```rust
use alloy_primitives::Address;
use alloy_provider::Provider;
use alloy_provider::fillers::NonceManager;
use alloy_transport::TransportResult;
```

**Nuestro fork** usa el crate umbrella:
```rust
use alloy::primitives::Address;
use alloy::providers::Provider;
use alloy::providers::fillers::NonceManager;
```

**Evaluacion**: Ambos enfoques son validos. Upstream usa crates individuales porque tienen un workspace multi-crate donde cada dependencia se controla granularmente. Nosotros usamos el crate umbrella `alloy = "1.0.12"` que re-exporta todo. No hay diferencia funcional.

#### 3. Integracion con retry logic

**Nuestro fork** tiene logica adicional de retry en `send_transaction()` (lineas 374-484) que upstream no tiene en su nonce manager file. Esta logica incluye:

- Proteccion anti-replay: verifica si el nonce confirmado on-chain avanzo antes de reintentar
- Funcion `is_nonce_error()` para clasificar errores de transporte
- Un solo reintento con backoff de 250ms
- Timeouts configurables por red (60s para Base, 30s para el resto)

Esto es codigo propio de Ultravioleta DAO que va mas alla de lo que upstream proporciona.

#### 4. Organizacion del archivo

**Upstream** tiene el nonce manager en su propio archivo (`pending_nonce_manager.rs`), lo cual es mas modular.

**Nuestro fork** lo tiene embebido al final de `evm.rs`, que tiene 1994 lineas. Esto funciona pero hace el archivo mas largo.

---

## 5. Plan de Implementacion

### ACCION REQUERIDA: NINGUNA

El `PendingNonceManager` ya esta completamente implementado y en produccion en nuestro fork. La implementacion es funcionalmente identica a la de upstream.

### Mejoras opcionales (baja prioridad)

Si en el futuro se desea mejorar la organizacion del codigo, se podria considerar:

#### Opcion A: Extraer a archivo separado (NO RECOMENDADO ahora)

Mover `PendingNonceManager` de `evm.rs` a un nuevo archivo `src/chain/pending_nonce_manager.rs` para mayor claridad modular.

**Pasos que se requeriran si se decide hacer:**

1. Crear `/mnt/z/ultravioleta/dao/x402-rs/src/chain/pending_nonce_manager.rs`:
```rust
//! Nonce management for concurrent EVM transaction submission.
//!
//! This module provides [`PendingNonceManager`], a custom nonce manager that improves
//! upon Alloy's default implementation by querying pending transactions when fetching
//! the initial nonce.

use alloy::primitives::Address;
use alloy::providers::Provider;
use alloy::providers::fillers::NonceManager;
use alloy::transports::TransportResult;
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Debug, Default)]
pub struct PendingNonceManager {
    nonces: Arc<DashMap<Address, Arc<Mutex<u64>>>>,
}

#[async_trait]
impl NonceManager for PendingNonceManager {
    async fn get_next_nonce<P, N>(
        &self,
        provider: &P,
        address: Address,
    ) -> TransportResult<u64>
    where
        P: Provider<N>,
        N: alloy::network::Network,
    {
        const NONE: u64 = u64::MAX;
        let nonce = {
            let rm = self
                .nonces
                .entry(address)
                .or_insert_with(|| Arc::new(Mutex::new(NONE)));
            Arc::clone(rm.value())
        };
        let mut nonce = nonce.lock().await;
        let new_nonce = if *nonce == NONE {
            tracing::trace!(%address, "fetching nonce");
            provider.get_transaction_count(address).pending().await?
        } else {
            tracing::trace!(%address, current_nonce = *nonce, "incrementing nonce");
            *nonce + 1
        };
        *nonce = new_nonce;
        Ok(new_nonce)
    }
}

impl PendingNonceManager {
    pub async fn reset_nonce(&self, address: Address) {
        if let Some(nonce_lock) = self.nonces.get(&address) {
            let mut nonce = nonce_lock.lock().await;
            *nonce = u64::MAX;
            tracing::debug!(%address, "reset nonce cache, will requery on next use");
        }
    }
}

#[cfg(test)]
mod tests {
    // Mover los 6 tests existentes de evm.rs aqui
}
```

2. Modificar `/mnt/z/ultravioleta/dao/x402-rs/src/chain/mod.rs` - agregar:
```rust
pub mod pending_nonce_manager;
```

3. Modificar `/mnt/z/ultravioleta/dao/x402-rs/src/chain/evm.rs`:
   - Eliminar lineas 1752-1994 (definicion de `PendingNonceManager` y tests)
   - Agregar import: `use crate::chain::pending_nonce_manager::PendingNonceManager;`
   - El resto del codigo (type aliases, EvmProvider, send_transaction) no cambia

**Por que NO se recomienda hacer esto ahora**: El refactor es puramente estetico y no aporta funcionalidad nueva. Introduce riesgo de errores de compilacion innecesarios. Solo tiene sentido si se va a hacer un merge grande de upstream donde la alineacion de estructura de archivos facilite la resolucion de conflictos.

---

## 6. Dependencias

### Dependencias ya presentes en nuestro Cargo.toml

| Dependencia | Version en fork | Version en upstream | Estado |
|-------------|----------------|--------------------|----|
| `dashmap` | `6.1.0` (linea 32) | `6.1.0` | Identica |
| `async-trait` | `0.1.88` (linea 31) | workspace (similar) | Compatible |
| `alloy` | `1.0.12` umbrella (linea 26) | `1.4` crates individuales | Nuestro es mas antiguo pero funcional |
| `tokio` | `1.49.0` (linea 17) | workspace | Compatible |

**Nota sobre version de Alloy**: Upstream usa Alloy 1.4 mientras nosotros usamos 1.0.12. El trait `NonceManager` y el metodo `.pending()` en `get_transaction_count()` existen en ambas versiones. No hay incompatibilidad.

**No se requiere agregar ninguna dependencia nueva.**

---

## 7. Evaluacion de Riesgos

### Riesgo: NINGUNO (feature ya implementada)

Como la feature ya esta implementada y en produccion, no hay riesgos de implementacion.

### Riesgos historicos mitigados por esta feature

Para referencia futura, estos son los problemas que `PendingNonceManager` resuelve:

| Riesgo | Descripcion | Mitigacion |
|--------|-------------|------------|
| Nonce too low tras restart | Transacciones pendientes en mempool causan nonce incorrecto | `.pending()` incluye transacciones del mempool |
| Race condition en nonces | Dos requests concurrentes obtienen el mismo nonce | `Mutex` por direccion previene asignacion duplicada |
| Nonce desfasado tras error | Error en TX deja cache con nonce incorrecto | `reset_nonce()` fuerza re-consulta al RPC |
| Double-spend en retry | Reintentar TX que ya fue minada por otro nodo | Proteccion anti-replay con verificacion de `post_nonce > pre_send_nonce` (solo en nuestro fork) |

### Riesgo futuro: merge de upstream

Cuando se haga merge de upstream en el futuro, habra conflicto en la zona del `PendingNonceManager` porque:

1. Upstream lo tiene en archivo separado (`pending_nonce_manager.rs`)
2. Nosotros lo tenemos embebido en `evm.rs`
3. La logica de retry en `send_transaction()` es nuestra y no existe en upstream

**Estrategia de merge recomendada**: Mantener nuestra estructura actual. En caso de conflicto, usar la version de nuestro fork ya que incluye la logica de retry anti-replay que upstream no tiene.

---

## 8. Checklist de Verificacion

Dado que la feature ya esta implementada, este checklist sirve para verificar que todo esta correcto:

### Verificacion de compilacion

- [x] `cargo build --release` compila sin errores
- [x] `PendingNonceManager` se usa en el type alias `InnerFiller` (linea 88)
- [x] `PendingNonceManager` se almacena en `EvmProvider` (linea 217)
- [x] `nonce_manager.clone()` se pasa al `NonceFiller` (linea 251)
- [x] `reset_nonce()` se invoca en ambos paths de error de `send_transaction()`

### Verificacion de tests

```bash
# Ejecutar tests del nonce manager
cargo test --lib -- tests::test_reset_nonce
cargo test --lib -- tests::test_is_nonce_error
cargo test --lib -- tests::test_multiple_addresses
cargo test --lib -- tests::test_concurrent_reset
```

### Verificacion en produccion

```bash
# Verificar que el facilitador arranca correctamente (los providers se inicializan con PendingNonceManager)
curl -s https://facilitator.ultravioletadao.xyz/health
# Esperado: {"status":"healthy"}

# Verificar que las redes EVM estan soportadas (cada una tiene su EvmProvider con PendingNonceManager)
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[].network] | unique | length'
```

### Verificacion funcional (requiere testnet)

```bash
# Enviar un pago de prueba en Base Sepolia
cd tests/integration
python test_usdc_payment.py --network base-sepolia

# Verificar logs para confirmar que PendingNonceManager funciona
# Buscar en CloudWatch: "fetching nonce" (primera consulta) o "incrementing nonce" (cache hit)
```

---

## 9. Conclusion

**El `PendingNonceManager` ya esta completamente adoptado en nuestro fork.** La implementacion es funcionalmente identica a la de upstream, con las siguientes ventajas adicionales de nuestro fork:

1. **Tracing siempre habilitado** - mejor observabilidad en produccion sin necesidad de feature flags
2. **Logica de retry anti-replay** - proteccion contra double-spend cuando un nodo RPC reporta error pero la TX ya fue minada
3. **Tests unitarios completos** - 6 tests que upstream no tiene en su archivo
4. **Timeouts configurables por red** - Base mainnet tiene 60s, otras redes 30s

**Accion requerida**: Ninguna. Cerrar este item como "ya implementado" en la lista de cherry-picks de upstream.

---

## Apendice: Lineas de Codigo Relevantes

| Archivo | Lineas | Contenido |
|---------|--------|-----------|
| `src/chain/evm.rs` | 24 | Import de `NonceManager` |
| `src/chain/evm.rs` | 37 | Import de `DashMap` |
| `src/chain/evm.rs` | 41 | Import de `Mutex` (tokio) |
| `src/chain/evm.rs` | 86-89 | Type alias `InnerFiller` con `NonceFiller<PendingNonceManager>` |
| `src/chain/evm.rs` | 95-98 | Type alias `InnerProvider` |
| `src/chain/evm.rs` | 217 | Campo `nonce_manager` en `EvmProvider` |
| `src/chain/evm.rs` | 242 | Construccion de `PendingNonceManager::default()` |
| `src/chain/evm.rs` | 246-255 | Construccion del filler stack con nonce manager |
| `src/chain/evm.rs` | 270 | Almacenamiento en struct `EvmProvider` |
| `src/chain/evm.rs` | 374 | `MAX_NONCE_RETRIES = 1` |
| `src/chain/evm.rs` | 429 | `reset_nonce()` en error de receipt |
| `src/chain/evm.rs` | 436 | `reset_nonce()` en error de envio |
| `src/chain/evm.rs` | 488-492 | `is_nonce_error()` helper |
| `src/chain/evm.rs` | 1752-1830 | Definicion completa de `PendingNonceManager` |
| `src/chain/evm.rs` | 1832-1994 | Tests unitarios (6 tests) |
| `Cargo.toml` | 32 | `dashmap = { version = "6.1.0" }` |
| `Cargo.toml` | 31 | `async-trait = { version = "0.1.88" }` |
