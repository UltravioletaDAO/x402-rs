# 07 - Soporte de Cadena Aptos

**Fecha**: 2026-02-12
**Prioridad**: Media-Alta
**Esfuerzo estimado**: 5-7 dias de desarrollo
**Riesgo**: Alto (dependencias nativas de aptos-core, patch de runtime requerido)
**Tipo**: Nueva cadena blockchain (Move-based L1)

---

## Tabla de Contenidos

1. [Resumen de la Funcionalidad](#1-resumen-de-la-funcionalidad)
2. [Analisis Upstream Completo](#2-analisis-upstream-completo)
3. [Plan de Adaptacion](#3-plan-de-adaptacion)
4. [Direcciones de Contratos USDC](#4-direcciones-de-contratos-usdc)
5. [Requisitos de Wallet](#5-requisitos-de-wallet)
6. [Actualizaciones del Frontend](#6-actualizaciones-del-frontend)
7. [Configuracion AWS](#7-configuracion-aws)
8. [Dependencias y Patches](#8-dependencias-y-patches)
9. [Evaluacion de Riesgo](#9-evaluacion-de-riesgo)
10. [Estimacion de Esfuerzo](#10-estimacion-de-esfuerzo)
11. [Checklist de Verificacion](#11-checklist-de-verificacion)

---

## 1. Resumen de la Funcionalidad

### Que es Aptos

Aptos es un blockchain Layer 1 basado en el lenguaje **Move** (originalmente desarrollado por Meta/Diem). Se distingue de las cadenas EVM y Solana en varios aspectos fundamentales:

- **Lenguaje Move**: Contratos inteligentes escritos en Move, no en Solidity ni Rust-SVM
- **Framework de Activos Fungibles**: USDC en Aptos usa `0x1::primary_fungible_store::transfer`, NO el patron ERC-20
- **Transacciones Sponsoreadas Nativas**: Aptos tiene soporte nativo de "fee payer transactions" a nivel de protocolo
- **Serializacion BCS**: Binary Canonical Serialization (no ABI encoding como EVM ni Borsh como Solana)
- **Direcciones de 32 bytes**: Formato hexadecimal con prefijo `0x`, 64 caracteres hex
- **Chain IDs simples**: `1` (mainnet), `2` (testnet)
- **Solo protocolo v2**: Usa exclusivamente CAIP-2 (`aptos:1`, `aptos:2`), sin soporte v1

### Que implemento upstream

Upstream agrego soporte completo para Aptos como un nuevo crate de cadena (`x402-chain-aptos`) en su arquitectura modular de workspace. La implementacion incluye:

1. **Crate `x402-chain-aptos`**: Modulo completo con provider, tipos, configuracion y facilitator
2. **Esquema `V2AptosExact`**: Verificacion y settlement de pagos exactos en Aptos
3. **Transacciones sponsoreadas**: El facilitador actua como fee payer (paga gas APT)
4. **Patch `aptos-runtimes`**: Hack necesario para compatibilidad con tokio 1.45+
5. **Patch `merlin`**: Fork de la libreria `merlin` de Aptos para compatibilidad
6. **Especificacion del esquema**: Documento completo en `docs/specs/schemes/exact/scheme_exact_aptos.md`

### Diferencia arquitectonica critica

Upstream usa una arquitectura de **workspace con crates separados por cadena**:

```
upstream:
  crates/chains/x402-chain-aptos/    <- Crate independiente
  crates/chains/x402-chain-eip155/   <- Crate independiente
  crates/chains/x402-chain-solana/   <- Crate independiente
  facilitator/                        <- Binario que combina todo via features
```

Nuestro fork usa una **arquitectura monolitica** con todo en `src/`:

```
nuestro fork:
  src/chain/evm.rs      <- Modulo dentro del crate raiz
  src/chain/solana.rs   <- Modulo dentro del crate raiz
  src/chain/sui.rs      <- Modulo dentro del crate raiz
  src/chain/aptos.rs    <- NUEVO: Modulo a crear
  src/network.rs         <- Enum Network centralizado
```

Esto significa que **no podemos hacer merge directo** del crate upstream. Debemos **portar manualmente** la logica del crate `x402-chain-aptos` a nuestro patron monolitico, similar a como se hizo con Sui.

---

## 2. Analisis Upstream Completo

### 2.1 Especificacion del Esquema (`scheme_exact_aptos.md`)

La especificacion define el flujo completo del protocolo para pagos exactos en Aptos:

#### Flujo del Protocolo

```
1. Cliente solicita recurso -> recibe 402 con PaymentRequirements
2. Cliente construye transaccion fee-payer con fee_payer = 0x0 (placeholder)
3. Cliente firma la transaccion (firma cubre payload, NO fee payer address)
4. Cliente serializa con BCS y codifica en Base64
5. Cliente reenvia peticion con PAYMENT-SIGNATURE header
6. Resource server pasa payload al facilitador para verificacion
7. Facilitador valida estructura, firma y detalles de pago
8. Resource server cumple la peticion
9. Resource server solicita settlement
10. Facilitador inyecta su address como fee payer, firma como sponsor
11. Facilitador envia transaccion fully-signed a la red Aptos
12. Facilitador reporta resultado (tx hash)
```

**Punto critico de seguridad**: El mecanismo de sponsorship NO da al fee payer posesion ni capacidad de alterar la transaccion del cliente. La firma del cliente cubre todo el payload (destinatario, cantidad, asset). El fee payer solo puede agregar su propia firma. Cualquier intento de modificar la transaccion invalida la firma del cliente.

#### Formato de Red

- **Mainnet**: `aptos:1` (CAIP-2)
- **Testnet**: `aptos:2` (CAIP-2)

#### PaymentRequirements

```json
{
  "scheme": "exact",
  "network": "aptos:1",
  "amount": "1000000",
  "asset": "0xbae207659db88bea0cbead6da0ed00aac12edcdda169e591cd41c94180b46f3b",
  "payTo": "0x1234...abcdef",
  "maxTimeoutSeconds": 60,
  "extra": {
    "sponsored": true
  }
}
```

#### Payload de Pago

```json
{
  "transaction": "AQDy8fLy8v..."  // Base64 de JSON con BCS transaction + authenticator
}
```

**Nota importante**: El campo `transaction` NO es BCS directo. Es Base64 de un JSON que contiene dos campos:
- `transaction`: Array de bytes BCS de la `RawTransaction` (posiblemente con fee payer suffix)
- `senderAuthenticator`: Array de bytes BCS del `AccountAuthenticator`

#### Esquemas de Firma Soportados

- **Ed25519**: Firma simple (mas comun)
- **MultiEd25519**: Multi-firma
- **SingleKey**: Ed25519, Secp256k1, o Secp256r1
- **MultiKey**: Multiples claves de diferentes tipos

### 2.2 Facilitator (`v2_aptos_exact/facilitator.rs`)

Este es el archivo central de la implementacion. Contiene ~270 lineas con tres funciones principales.

#### Estructura `V2AptosExactFacilitator`

```rust
pub struct V2AptosExactFacilitator {
    provider: Arc<AptosChainProvider>,
}
```

Implementa el trait `X402SchemeFacilitator` con tres metodos: `verify`, `settle`, `supported`.

#### Funcion `verify_transfer` (Verificacion)

La verificacion sigue estos pasos exactos:

1. **Validar accepted == requirements**: Comparacion directa de PaymentRequirements
2. **Validar chain ID**: Comparar chain ID del provider con el del payload
3. **Deserializar transaccion**: Decodificar Base64 -> JSON -> BCS
4. **Extraer sender (payer)**: `raw_transaction.sender()`
5. **Validar entry function**: Debe ser `0x1::primary_fungible_store::transfer`
   - Modulo: `AccountAddress::ONE` + `"primary_fungible_store"`
   - Funcion: `"transfer"`
6. **Validar argumentos** (3 exactos):
   - `args[0]`: Asset address (BCS-encoded `AccountAddress`) -> debe coincidir con `requirements.asset`
   - `args[1]`: Recipient address (BCS-encoded `AccountAddress`) -> debe coincidir con `requirements.pay_to`
   - `args[2]`: Amount (BCS-encoded `u64`) -> debe coincidir con `requirements.amount`

```rust
pub struct VerifyTransferResult {
    pub payer: AccountAddress,
    pub raw_transaction: RawTransaction,
    pub authenticator_bytes: Vec<u8>,
}
```

#### Funcion `settle_transaction` (Settlement)

El settlement tiene dos modos:

**Modo Sponsoreado** (`sponsor_gas = true`):
1. Deserializar `AccountAuthenticator` del sender
2. Obtener fee payer address y private key del provider
3. Crear `RawTransactionWithData::new_fee_payer` con la raw transaction + fee payer address
4. Firmar como fee payer con Ed25519
5. Construir `SignedTransaction::new_fee_payer` con:
   - Raw transaction
   - Sender authenticator
   - Sin signers secundarios
   - Fee payer address
   - Fee payer authenticator
6. Calcular `committed_hash()` para obtener tx hash
7. Enviar via `rest_client.submit_bcs()`

**Modo No-Sponsoreado** (`sponsor_gas = false`):
1. Extraer public key y signature del authenticator (solo Ed25519 soportado)
2. Crear `SignedTransaction::new` estandar
3. Enviar directamente

#### Funcion `deserialize_aptos_transaction`

Esta funcion es particularmente compleja. El proceso es:

1. **Base64 decode** del string `transaction`
2. **JSON parse** del resultado (NO es BCS directo)
3. Extraer campo `"transaction"` como array de u8
4. Extraer campo `"senderAuthenticator"` como array de u8
5. **Deteccion de fee payer transaction**: Si los ultimos 33 bytes tienen `[1]` como tag de Option, es una fee payer transaction y se truncan esos 33 bytes antes de deserializar
6. **BCS deserialize** de `RawTransaction`
7. Extraer `EntryFunction` del payload

```rust
// Logica de deteccion de fee payer:
if transaction_bytes.len() > 33 {
    let maybe_option_tag = transaction_bytes[transaction_bytes.len() - 33];
    if maybe_option_tag == 1 {
        // Fee payer tx - truncar ultimos 33 bytes (Option<Address>)
        let raw_tx_bytes = &transaction_bytes[..transaction_bytes.len() - 33];
        bcs::from_bytes(raw_tx_bytes)?
    }
}
```

**Esto es un hack fragil** que depende de la estructura interna de serializacion BCS de Aptos. Funciona porque:
- Una `Option<AccountAddress>` serializada como `Some(address)` ocupa exactamente 33 bytes: 1 byte tag `1` + 32 bytes address
- El placeholder `0x0` se serializa como 32 bytes de ceros

### 2.3 Provider (`chain/provider.rs`)

El `AptosChainProvider` encapsula la conexion a la red Aptos:

```rust
pub struct AptosChainProvider {
    chain: AptosChainReference,        // aptos:1 o aptos:2
    sponsor_gas: bool,                  // Si actua como fee payer
    fee_payer_address: Option<AccountAddress>,  // Address derivada de la key
    fee_payer_private_key: Option<Ed25519PrivateKey>,  // Key para firmar
    rest_client: Arc<AptosClient>,      // Cliente REST API de Aptos
}
```

#### Inicializacion desde Config

```rust
pub async fn from_config(config: &AptosChainConfig) -> Result<Self, Box<dyn std::error::Error>> {
    // 1. Validar: si sponsor_gas=true, signer es requerido
    // 2. Parsear private key hex -> Ed25519PrivateKey
    // 3. Derivar account address: private_key -> public_key -> AuthenticationKey -> account_address
    // 4. Crear REST client (con API key opcional como Bearer token)
}
```

**Derivacion de address**: A diferencia de EVM (donde la address se deriva del hash keccak256 de la public key), Aptos usa `AuthenticationKey::ed25519(&public_key).account_address()`.

#### API Key como Bearer Token

El provider soporta API keys opcionales que se envian como `Authorization: Bearer {api_key}` con todas las peticiones RPC. Esto es util para servicios como Alchemy o endpoints protegidos.

### 2.4 Types (`chain/types.rs`)

Define tipos fundamentales para Aptos:

- **`AptosChainReference`**: Wrapper de u8 para chain IDs (1=mainnet, 2=testnet)
- **`Address`**: Wrapper de `AccountAddress` con serializacion hex `0x`-prefixed
- **`AptosTokenDeployment`**: Chain reference + address + decimals

La conversion a `ChainId` (CAIP-2) es directa:
```rust
impl From<AptosChainReference> for ChainId {
    fn from(value: AptosChainReference) -> Self {
        ChainId::new("aptos", value.0.to_string())  // "aptos:1" o "aptos:2"
    }
}
```

### 2.5 Networks (`networks.rs`)

Define el trait `KnownNetworkAptos` y las implementaciones de USDC:

```rust
pub trait KnownNetworkAptos<A> {
    fn aptos() -> A;
    fn aptos_testnet() -> A;
}

// USDC en Aptos mainnet
impl KnownNetworkAptos<AptosTokenDeployment> for USDC {
    fn aptos() -> AptosTokenDeployment {
        let address: Address = "0xbae207659db88bea0cbead6da0ed00aac12edcdda169e591cd41c94180b46f3b"
            .parse().expect("Invalid USDC address");
        AptosTokenDeployment::new(AptosChainReference::aptos(), address, 6)
    }

    fn aptos_testnet() -> AptosTokenDeployment {
        // Misma address que mainnet (placeholder, puede cambiar)
        let address: Address = "0xbae207659db88bea0cbead6da0ed00aac12edcdda169e591cd41c94180b46f3b"
            .parse().expect("Invalid USDC address");
        AptosTokenDeployment::new(AptosChainReference::aptos_testnet(), address, 6)
    }
}
```

### 2.6 Config (`chain/config.rs`)

Sistema de configuracion con soporte para variables de entorno via `LiteralOrEnv`:

```rust
pub struct AptosChainConfigInner {
    pub rpc: LiteralOrEnv<Url>,                     // RPC URL (requerido)
    pub api_key: Option<LiteralOrEnv<String>>,       // API key (opcional)
    pub signer: Option<AptosSignerConfig>,           // Private key (requerido si sponsor_gas)
    pub sponsor_gas: LiteralOrEnv<bool>,             // Default: false
}
```

Ejemplo de configuracion JSON:
```json
{
  "chains": {
    "aptos:1": {
      "rpc": "$APTOS_RPC_URL",
      "api_key": "$APTOS_API_KEY",
      "sponsor_gas": "$APTOS_SPONSOR_GAS",
      "signer": { "private_key": "$APTOS_PRIVATE_KEY" }
    }
  }
}
```

La `AptosPrivateKey` soporta claves de 32 bytes (Ed25519 seed) o 64 bytes (keypair completo).

### 2.7 Cargo.toml del Crate

Dependencias criticas:

```toml
[dependencies]
move-core-types = { git = "https://github.com/aptos-labs/aptos-core", tag = "aptos-node-v1.39.2" }

# Solo con feature "facilitator":
aptos-crypto = { git = "...", tag = "aptos-node-v1.39.2", optional = true }
aptos-rest-client = { git = "...", tag = "aptos-node-v1.39.2", optional = true }
aptos-types = { git = "...", tag = "aptos-node-v1.39.2", optional = true }
bcs = { version = "0.1", optional = true }
hex = { version = "0.4", optional = true }
```

**IMPORTANTE**: Todas las dependencias de Aptos vienen del repositorio Git `aptos-labs/aptos-core` con tag `aptos-node-v1.39.2`. NO estan en crates.io (por eso `publish = false`).

### 2.8 Patches Requeridos

#### Patch `aptos-runtimes`

El workspace upstream incluye un patch critico:

```toml
[patch."https://github.com/aptos-labs/aptos-core"]
aptos-runtimes = { path = "patches/aptos-runtimes" }
```

El crate `aptos-runtimes` original de aptos-core llama a `disable_lifo_slot()` en el builder de tokio, que fue **removido en tokio 1.45+**. El patch simplemente elimina esa llamada:

```rust
// patches/aptos-runtimes/src/lib.rs
// NOTE: disable_lifo_slot() removed for tokio 1.45+ compatibility
let mut builder = Builder::new_multi_thread();
builder
    .thread_name_fn(...)
    .on_thread_start(on_thread_start)
    // disable_lifo_slot() REMOVIDO
    .max_blocking_threads(MAX_BLOCKING_THREADS)
    .enable_all();
```

El patch es 72 lineas de codigo (incluyendo un thread pool de rayon).

#### Patch `merlin`

```toml
[patch.crates-io]
merlin = { git = "https://github.com/aptos-labs/merlin" }
```

`merlin` es una libreria de transcript para protocolos criptograficos. Aptos mantiene un fork porque la version original no es compatible con su stack.

### 2.9 Integracion en el Facilitator Upstream

En el binario facilitator upstream, Aptos se integra asi:

**`facilitator/src/run.rs`**:
```rust
#[cfg(feature = "chain-aptos")]
use x402_chain_aptos::V2AptosExact;

// En la funcion run():
#[cfg(feature = "chain-aptos")]
{
    scheme_blueprints.register(V2AptosExact);
}
```

**`facilitator/src/chain.rs`** - Enum `ChainProvider`:
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

**`facilitator/src/config.rs`** - Deserializacion por namespace:
```rust
#[cfg(feature = "chain-aptos")]
aptos::APTOS_NAMESPACE => {
    let inner: AptosChainConfigInner = access.next_value()?;
    let config = AptosChainConfig { chain_reference: chain_id.try_into()?, inner };
    ChainConfig::Aptos(Box::new(config))
}
```

**`facilitator/src/schemes.rs`** - Builder adapter:
```rust
#[cfg(feature = "chain-aptos")]
impl X402SchemeFacilitatorBuilder<&ChainProvider> for V2AptosExact {
    fn build(&self, provider: &ChainProvider, config: Option<serde_json::Value>)
        -> Result<Box<dyn X402SchemeFacilitator>, Box<dyn std::error::Error>>
    {
        let aptos_provider = if let ChainProvider::Aptos(provider) = provider {
            Arc::clone(provider)
        } else {
            return Err("...");
        };
        self.build(aptos_provider, config)
    }
}
```

---

## 3. Plan de Adaptacion

### 3.1 Estrategia General

Seguiremos el **mismo patron que Sui**: feature-gated con `#[cfg(feature = "aptos")]`, nuevo modulo en `src/chain/aptos.rs`, nuevas variantes en `Network` enum, y variables de entorno dedicadas.

La diferencia principal con Sui es que Aptos requiere **patches de workspace** que afectan el arbol de dependencias completo.

### 3.2 Nuevos Archivos

#### `src/chain/aptos.rs` (~500-600 lineas estimadas)

Este sera el archivo principal. Debe portar la logica de:
- `x402-chain-aptos/src/chain/provider.rs` -> Struct `AptosProvider`
- `x402-chain-aptos/src/v2_aptos_exact/facilitator.rs` -> Impl `Facilitator` trait
- `x402-chain-aptos/src/v2_aptos_exact/types.rs` -> Tipos del payload

```rust
//! Aptos blockchain payment verification and settlement.
//!
//! This module implements x402 payment flows for the Aptos blockchain using
//! sponsored transactions. Aptos provides protocol-level gas sponsorship (fee payer),
//! allowing the facilitator to pay gas fees while users pay only the
//! stablecoin transfer amount.
//!
//! # Sponsored Transaction Flow
//!
//! 1. Client constructs a fungible asset transfer transaction
//! 2. Client signs the transaction (covering payload, not fee payer)
//! 3. Client sends Base64-encoded BCS transaction to facilitator
//! 4. Facilitator verifies transaction parameters and signature
//! 5. Facilitator adds fee payer address and co-signs
//! 6. Facilitator submits the sponsored transaction to Aptos network
//!
//! # USDC on Aptos
//!
//! Aptos USDC uses 6 decimals (same as EVM chains):
//! - Mainnet: 0xbae207659db88bea0cbead6da0ed00aac12edcdda169e591cd41c94180b46f3b
//! - Testnet: (to be verified - upstream uses same address as placeholder)

use aptos_crypto::ed25519::{Ed25519PrivateKey, Ed25519PublicKey};
use aptos_crypto::SigningKey;
use aptos_rest_client::Client as AptosClient;
use aptos_types::transaction::authenticator::{AccountAuthenticator, AuthenticationKey};
use aptos_types::transaction::{EntryFunction, RawTransaction, RawTransactionWithData, SignedTransaction};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::ModuleId;
use tracing::{debug, error, info, warn};

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env::{
    ENV_RPC_APTOS, ENV_RPC_APTOS_TESTNET,
    ENV_APTOS_PRIVATE_KEY, ENV_APTOS_PRIVATE_KEY_MAINNET, ENV_APTOS_PRIVATE_KEY_TESTNET,
};
use crate::network::Network;
use crate::types::{...};

/// USDC fungible asset metadata address on Aptos mainnet
pub const USDC_ASSET_MAINNET: &str =
    "0xbae207659db88bea0cbead6da0ed00aac12edcdda169e591cd41c94180b46f3b";

/// USDC fungible asset metadata address on Aptos testnet
/// NOTE: Verificar address real de testnet antes de produccion
pub const USDC_ASSET_TESTNET: &str =
    "0xbae207659db88bea0cbead6da0ed00aac12edcdda169e591cd41c94180b46f3b";

pub struct AptosProvider {
    network: Network,
    rpc_url: String,
    sponsor_gas: bool,
    fee_payer_address: Option<AccountAddress>,
    fee_payer_private_key: Option<Ed25519PrivateKey>,
    rest_client: AptosClient,
    usdc_asset: String,
}
```

La implementacion de `Facilitator` para `AptosProvider` debera:

1. **`verify()`**: Deserializar la transaccion BCS del payload, validar que sea `primary_fungible_store::transfer` con los argumentos correctos (asset, recipient, amount)
2. **`settle()`**: Verificar primero, luego firmar como fee payer y enviar
3. **`supported()`**: Retornar `SupportedPaymentKind` con `extra.sponsored = true`

#### `patches/aptos-runtimes/` (directorio nuevo)

Copiar directamente de upstream:

- `patches/aptos-runtimes/Cargo.toml` (5 lineas)
- `patches/aptos-runtimes/src/lib.rs` (72 lineas)

Este patch es **obligatorio** porque `aptos-rest-client` depende transitivamente de `aptos-runtimes`, y sin el patch la compilacion falla con tokio >= 1.45.

### 3.3 Modificaciones a Archivos Existentes

#### `Cargo.toml` - Dependencias nuevas

```toml
# Aptos (Move-based L1 with sponsored transactions)
aptos-crypto = { git = "https://github.com/aptos-labs/aptos-core", tag = "aptos-node-v1.39.2", optional = true }
aptos-rest-client = { git = "https://github.com/aptos-labs/aptos-core", tag = "aptos-node-v1.39.2", optional = true }
aptos-types = { git = "https://github.com/aptos-labs/aptos-core", tag = "aptos-node-v1.39.2", optional = true }
move-core-types = { git = "https://github.com/aptos-labs/aptos-core", tag = "aptos-node-v1.39.2", optional = true }

# BCS ya existe como opcional para Sui, compartir con Aptos
# bcs = { version = "0.1", optional = true }  # Ya existe

# Features
[features]
aptos = ["aptos-crypto", "aptos-rest-client", "aptos-types", "move-core-types", "bcs"]

# Patches CRITICOS
[patch.crates-io]
merlin = { git = "https://github.com/aptos-labs/merlin" }

[patch."https://github.com/aptos-labs/aptos-core"]
aptos-runtimes = { path = "patches/aptos-runtimes" }
```

**ADVERTENCIA CRITICA**: El patch de `merlin` en `[patch.crates-io]` podria afectar a TODAS las dependencias del workspace que usen `merlin`, no solo a las de Aptos. Hay que verificar que ni Sui SDK ni otras dependencias requieran `merlin` original.

El `bcs` crate ya existe como dependencia opcional para Sui. Hay que verificar compatibilidad de version:
- Sui usa `bcs = { version = "0.1" }` (de MystenLabs/sui via git)
- Aptos usa `bcs = { version = "0.1" }` (de crates.io)

Estos PODRIAN ser crates diferentes con el mismo nombre. Si hay conflicto, habra que resolver con features separados.

#### `src/network.rs` - Nuevas variantes

```rust
// En el enum Network:
/// Aptos mainnet (chain ID 1, Move-based L1).
#[cfg(feature = "aptos")]
#[serde(rename = "aptos")]
Aptos,
/// Aptos testnet (chain ID 2, Move-based L1).
#[cfg(feature = "aptos")]
#[serde(rename = "aptos-testnet")]
AptosTestnet,

// En Display:
#[cfg(feature = "aptos")]
Network::Aptos => write!(f, "aptos"),
#[cfg(feature = "aptos")]
Network::AptosTestnet => write!(f, "aptos-testnet"),

// En FromStr:
"aptos" => Ok(Network::Aptos),
"aptos-testnet" => Ok(Network::AptosTestnet),

// En is_testnet():
#[cfg(feature = "aptos")]
Network::AptosTestnet => true,
#[cfg(feature = "aptos")]
Network::Aptos => false,

// En NetworkFamily:
#[cfg(feature = "aptos")]
Aptos,

// En From<Network> for NetworkFamily:
#[cfg(feature = "aptos")]
Network::Aptos => NetworkFamily::Aptos,
#[cfg(feature = "aptos")]
Network::AptosTestnet => NetworkFamily::Aptos,
```

Tambien agregar chain IDs y USDC deployments como constantes estaticas:

```rust
// No hay chain IDs EVM para Aptos - usa su propio sistema
// Aptos Chain ID 1 = mainnet, 2 = testnet

pub static USDC_APTOS: Lazy<TokenDeployment> = Lazy::new(|| {
    TokenDeployment {
        network: Network::Aptos,
        token_type: TokenType::USDC,
        asset: TokenAsset::Aptos("0xbae207659db88bea0cbead6da0ed00aac12edcdda169e591cd41c94180b46f3b".to_string()),
        decimals: 6,
        eip712: None,  // No aplica para Aptos
    }
});
```

**Nota**: Necesitaremos extender `TokenAsset` (o `MixedAddress`) para soportar direcciones Aptos de 32 bytes.

#### `src/from_env.rs` - Variables de Entorno

```rust
// Aptos RPC URLs
#[cfg(feature = "aptos")]
pub const ENV_RPC_APTOS: &str = "RPC_URL_APTOS";
#[cfg(feature = "aptos")]
pub const ENV_RPC_APTOS_TESTNET: &str = "RPC_URL_APTOS_TESTNET";

// Aptos wallet private key environment variables
#[cfg(feature = "aptos")]
pub const ENV_APTOS_PRIVATE_KEY: &str = "APTOS_PRIVATE_KEY";
#[cfg(feature = "aptos")]
pub const ENV_APTOS_PRIVATE_KEY_MAINNET: &str = "APTOS_PRIVATE_KEY_MAINNET";
#[cfg(feature = "aptos")]
pub const ENV_APTOS_PRIVATE_KEY_TESTNET: &str = "APTOS_PRIVATE_KEY_TESTNET";

// En rpc_env_name_from_network():
#[cfg(feature = "aptos")]
Network::Aptos => ENV_RPC_APTOS,
#[cfg(feature = "aptos")]
Network::AptosTestnet => ENV_RPC_APTOS_TESTNET,
```

#### `src/chain/mod.rs` - Nuevo provider

```rust
#[cfg(feature = "aptos")]
pub mod aptos;

// En enum NetworkProvider:
#[cfg(feature = "aptos")]
Aptos(aptos::AptosProvider),

// En FromEnvByNetworkBuild:
#[cfg(feature = "aptos")]
NetworkFamily::Aptos => {
    let provider = aptos::AptosProvider::from_env(network).await?;
    provider.map(NetworkProvider::Aptos)
}

// En NetworkProviderOps (signer_address, network):
#[cfg(feature = "aptos")]
NetworkProvider::Aptos(provider) => provider.signer_address(),
// ... etc para cada metodo del trait

// En Facilitator impl:
#[cfg(feature = "aptos")]
NetworkProvider::Aptos(provider) => provider.verify(request).await,
// ... etc para verify, settle, supported
```

#### `src/caip2.rs` - Namespace Aptos

```rust
// En enum Namespace:
/// Aptos blockchain (Move-based L1).
/// Reference is the chain ID as string ("1" or "2").
#[cfg(feature = "aptos")]
Aptos,

// En Display:
#[cfg(feature = "aptos")]
Namespace::Aptos => write!(f, "aptos"),

// En FromStr:
#[cfg(feature = "aptos")]
"aptos" => Ok(Namespace::Aptos),

// En la funcion de conversion a Network:
// "aptos:1" -> Network::Aptos
// "aptos:2" -> Network::AptosTestnet
```

#### `src/types.rs` - Nuevos tipos de payload

Agregar variante para payload Aptos:

```rust
/// Aptos exact payment payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExactAptosPayload {
    /// Base64-encoded JSON containing BCS transaction and sender authenticator.
    pub transaction: String,
}

// En enum ExactPaymentPayload:
#[cfg(feature = "aptos")]
Aptos(ExactAptosPayload),

// En MixedAddress:
#[cfg(feature = "aptos")]
Aptos(String),
```

#### `src/handlers.rs` - Endpoint de logo (opcional)

Si se agrega logo de Aptos al landing page:

```rust
pub async fn get_aptos_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/aptos.png");
    (
        [(header::CONTENT_TYPE, "image/png")],
        bytes.as_slice(),
    )
}

// En el router:
.route("/aptos.png", get(get_aptos_logo))
```

#### `src/provider_cache.rs` - Soporte para Aptos

Verificar si `ProviderCache` necesita cambios para soportar el nuevo `NetworkFamily::Aptos`. Dado que usa `NetworkProvider` como tipo generico, deberia funcionar sin cambios si la inicializacion se hace correctamente.

#### `.env.example` - Nuevas variables

```bash
# Aptos RPC URLs
RPC_URL_APTOS=https://fullnode.mainnet.aptoslabs.com/v1
RPC_URL_APTOS_TESTNET=https://fullnode.testnet.aptoslabs.com/v1

# Aptos Facilitator Wallet (Ed25519 private key, hex-encoded with 0x prefix)
# Use separate keys for mainnet and testnet
APTOS_PRIVATE_KEY_MAINNET=
APTOS_PRIVATE_KEY_TESTNET=
```

### 3.4 Mapeo de Codigo Upstream -> Nuestro Fork

| Archivo Upstream | Destino en Nuestro Fork | Notas |
|---|---|---|
| `crates/chains/x402-chain-aptos/src/chain/provider.rs` | `src/chain/aptos.rs` (struct AptosProvider) | Simplificar config, usar env vars directas |
| `crates/chains/x402-chain-aptos/src/chain/types.rs` | `src/chain/aptos.rs` + `src/types.rs` | Integrar Address como MixedAddress::Aptos |
| `crates/chains/x402-chain-aptos/src/chain/config.rs` | `src/chain/aptos.rs` (metodo from_env) | Reemplazar config JSON por env vars |
| `crates/chains/x402-chain-aptos/src/v2_aptos_exact/facilitator.rs` | `src/chain/aptos.rs` (impl Facilitator) | Funcion principal a portar |
| `crates/chains/x402-chain-aptos/src/v2_aptos_exact/types.rs` | `src/types.rs` (ExactAptosPayload) | Tipos del payload |
| `crates/chains/x402-chain-aptos/src/networks.rs` | `src/network.rs` (USDC constants) | Addresses USDC |
| `crates/chains/x402-chain-aptos/src/lib.rs` | N/A (no necesario, el modulo se integra directamente) | Solo re-exports |
| `patches/aptos-runtimes/` | `patches/aptos-runtimes/` (copiar directamente) | Obligatorio |

---

## 4. Direcciones de Contratos USDC

### Mainnet (aptos:1)

| Token | Metadata Address | Decimales | Verificado |
|---|---|---|---|
| USDC | `0xbae207659db88bea0cbead6da0ed00aac12edcdda169e591cd41c94180b46f3b` | 6 | Si (upstream) |

**Nota**: En Aptos, las direcciones de tokens son **metadata addresses** del framework de activos fungibles, no direcciones de contrato como en EVM. La transferencia se hace via `0x1::primary_fungible_store::transfer` pasando esta metadata address como primer argumento.

### Testnet (aptos:2)

| Token | Metadata Address | Decimales | Verificado |
|---|---|---|---|
| USDC | `0xbae207659db88bea0cbead6da0ed00aac12edcdda169e591cd41c94180b46f3b` | 6 | **NO** - upstream usa misma address como placeholder |

**ACCION REQUERIDA**: Antes de desplegar testnet, verificar la address real de USDC en Aptos testnet. La address de mainnet podria no existir en testnet. Verificar en:
- https://explorer.aptoslabs.com/?network=testnet
- Circle docs para Aptos USDC testnet faucet

### Metodo de Transferencia

A diferencia de EVM (ERC-20 `transferWithAuthorization`) y Solana (SPL Token transfer), Aptos usa:

```move
0x1::primary_fungible_store::transfer<T: key>(
    sender: &signer,
    metadata: Object<T>,   // <- Address del asset (USDC metadata)
    recipient: address,     // <- Destinatario
    amount: u64,            // <- Cantidad en unidades atomicas
)
```

Alternativa mas eficiente (pero requiere stores existentes):
```move
0x1::fungible_asset::transfer<T: key>(
    sender: &signer,
    from: Object<T>,    // <- Store object del sender
    to: Object<T>,      // <- Store object del recipient
    amount: u64,
)
```

El facilitador valida que la transaccion use `primary_fungible_store::transfer` (la primera opcion), que es mas segura porque auto-crea stores si no existen.

---

## 5. Requisitos de Wallet

### Tipo de Wallet

- **Algoritmo**: Ed25519 (esquema principal de Aptos)
- **Formato de clave privada**: Hex-encoded, 32 bytes (seed) o 64 bytes (keypair completo), con prefijo `0x`
- **Derivacion de address**: `private_key -> public_key -> AuthenticationKey::ed25519() -> account_address()`

### Tokens Necesarios

| Red | Token Nativo | Proposito | Cantidad Sugerida |
|---|---|---|---|
| Aptos Mainnet | APT | Gas para sponsored transactions | 5-10 APT (~$50-100 USD) |
| Aptos Testnet | APT (testnet) | Gas para pruebas | Faucet gratuito |

**NO necesita USDC**. El facilitador solo paga gas (APT). Los pagos USDC fluyen directamente del cliente al resource server.

### Generacion de Wallet

```bash
# Instalar Aptos CLI
curl -fsSL "https://aptos.dev/scripts/install_cli.py" | python3

# Generar nueva cuenta
aptos init --network mainnet
# Esto genera ~/.aptos/config.yaml con la private key

# O generar programaticamente (Python)
from aptos_sdk.account import Account
account = Account.generate()
print(f"Private key: 0x{account.private_key.hex()}")
print(f"Address: {account.address()}")
```

### Almacenamiento de Claves

Las claves de Aptos deben almacenarse en AWS Secrets Manager:

- `facilitator-aptos-private-key-mainnet` - Clave privada mainnet
- `facilitator-aptos-private-key-testnet` - Clave privada testnet

---

## 6. Actualizaciones del Frontend

### Logo de Aptos

Crear/obtener `static/aptos.png`:
- Formato: PNG
- Tamano: Consistente con los otros logos de red (~64x64 o ~128x128)
- Fuente: Logo oficial de Aptos (disponible en https://aptosfoundation.org/brand)

### `static/index.html` - Tarjeta de Red

Agregar tarjeta de red en la seccion de redes soportadas:

```html
<!-- Network card para Aptos -->
<div class="network-card" data-network="aptos">
    <img src="/aptos.png" alt="Aptos" class="network-logo" loading="lazy" />
    <span class="network-name" data-i18n="network.aptos">Aptos</span>
    <span class="network-chain-id">Chain ID: 1</span>
    <span class="network-type">Move L1</span>
</div>
```

Y agregar testnet card:

```html
<div class="network-card testnet" data-network="aptos-testnet">
    <img src="/aptos.png" alt="Aptos Testnet" class="network-logo" loading="lazy" />
    <span class="network-name" data-i18n="network.aptos-testnet">Aptos Testnet</span>
    <span class="network-chain-id">Chain ID: 2</span>
    <span class="network-type">Move L1 Testnet</span>
</div>
```

### Traducciones i18n

En el bloque de traducciones de `index.html`:

```javascript
// English
"network.aptos": "Aptos",
"network.aptos-testnet": "Aptos Testnet",

// Spanish
"network.aptos": "Aptos",
"network.aptos-testnet": "Aptos Testnet",
```

### CSS para Logo

Agregar estilos si es necesario (probablemente los estilos genericos de `.network-card` ya cubren esto).

### `src/handlers.rs` - Ruta del Logo

```rust
.route("/aptos.png", get(get_aptos_logo))
```

---

## 7. Configuracion AWS

### AWS Secrets Manager

#### Nuevos Secretos

| Nombre del Secreto | Tipo | Contenido |
|---|---|---|
| `facilitator-aptos-private-key-mainnet` | SecureString | `0x{hex_private_key}` |
| `facilitator-aptos-private-key-testnet` | SecureString | `0x{hex_private_key}` |
| `facilitator-rpc-mainnet` | JSON (actualizar) | Agregar campo `"aptos": "https://fullnode.mainnet.aptoslabs.com/v1"` |
| `facilitator-rpc-testnet` | JSON (actualizar) | Agregar campo `"aptos-testnet": "https://fullnode.testnet.aptoslabs.com/v1"` |

#### Creacion de Secretos

```bash
# Aptos mainnet private key
aws secretsmanager create-secret \
  --name facilitator-aptos-private-key-mainnet \
  --description "Aptos mainnet facilitator Ed25519 private key" \
  --secret-string "0x{PRIVATE_KEY_HEX}" \
  --region us-east-2

# Aptos testnet private key
aws secretsmanager create-secret \
  --name facilitator-aptos-private-key-testnet \
  --description "Aptos testnet facilitator Ed25519 private key" \
  --secret-string "0x{PRIVATE_KEY_HEX}" \
  --region us-east-2

# Actualizar secreto RPC mainnet
aws secretsmanager get-secret-value \
  --secret-id facilitator-rpc-mainnet \
  --region us-east-2 \
  --query SecretString --output text | \
  jq '. + {"aptos": "https://fullnode.mainnet.aptoslabs.com/v1"}' | \
  aws secretsmanager update-secret \
    --secret-id facilitator-rpc-mainnet \
    --region us-east-2 \
    --secret-string file:///dev/stdin
```

### Terraform - Task Definition

Agregar variables de entorno y secretos en `terraform/environments/production/main.tf`:

```hcl
# En la seccion de environment del container:
{
  name  = "RPC_URL_APTOS"
  value = "https://fullnode.mainnet.aptoslabs.com/v1"
},
{
  name  = "RPC_URL_APTOS_TESTNET"
  value = "https://fullnode.testnet.aptoslabs.com/v1"
},

# En la seccion de secrets del container:
{
  name      = "APTOS_PRIVATE_KEY_MAINNET"
  valueFrom = "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-aptos-private-key-mainnet-XXXXXX"
},
{
  name      = "APTOS_PRIVATE_KEY_TESTNET"
  valueFrom = "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-aptos-private-key-testnet-XXXXXX"
},
```

**Nota sobre RPC**: Las URLs publicas de Aptos (`fullnode.mainnet.aptoslabs.com`) no contienen API keys, por lo que pueden ir directamente en `environment` (no en `secrets`). Si se usa un proveedor premium (Alchemy, etc.), usar `secrets` con referencia a Secrets Manager.

### Permisos IAM

Actualizar la politica IAM del task role para permitir acceso a los nuevos secretos:

```json
{
  "Effect": "Allow",
  "Action": "secretsmanager:GetSecretValue",
  "Resource": [
    "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-aptos-private-key-mainnet-*",
    "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-aptos-private-key-testnet-*"
  ]
}
```

---

## 8. Dependencias y Patches

### Dependencias Directas

| Crate | Version/Tag | Fuente | Proposito |
|---|---|---|---|
| `aptos-crypto` | `aptos-node-v1.39.2` | Git (aptos-labs/aptos-core) | Criptografia Ed25519, firma |
| `aptos-types` | `aptos-node-v1.39.2` | Git (aptos-labs/aptos-core) | Tipos de transaccion, RawTransaction |
| `aptos-rest-client` | `aptos-node-v1.39.2` | Git (aptos-labs/aptos-core) | Cliente REST API para enviar txs |
| `move-core-types` | `aptos-node-v1.39.2` | Git (aptos-labs/aptos-core) | AccountAddress, ModuleId, Identifier |
| `bcs` | `0.1` | crates.io | Binary Canonical Serialization |
| `hex` | `0.4` | crates.io | Ya existe como dependencia |

### Dependencias Transitivas Criticas

El arbol de dependencias de `aptos-core` es **enorme**. Las dependencias transitivas incluyen:

- `aptos-runtimes` (necesita patch)
- `merlin` (necesita patch/fork)
- `rayon` (thread pools)
- Multiples crates de criptografia (curve25519-dalek, ed25519-dalek, etc.)
- `reqwest` (ya existe, pero verificar version)

### Patches Obligatorios

#### 1. Patch `aptos-runtimes`

**Razon**: `aptos-runtimes` llama a `disable_lifo_slot()` que fue removido en tokio >= 1.45. Nuestro proyecto usa tokio 1.49.

**Implementacion**: Crear directorio `patches/aptos-runtimes/` con:

`patches/aptos-runtimes/Cargo.toml`:
```toml
[package]
name = "aptos-runtimes"
version = "0.1.0"
edition = "2021"

[dependencies]
rayon = "1.10"
tokio = { version = "1.35", features = ["rt-multi-thread"] }
```

`patches/aptos-runtimes/src/lib.rs`: Copiar directamente de upstream (72 lineas). La clave es que **NO** llama a `disable_lifo_slot()`.

**En Cargo.toml raiz**:
```toml
[patch."https://github.com/aptos-labs/aptos-core"]
aptos-runtimes = { path = "patches/aptos-runtimes" }
```

#### 2. Patch `merlin`

**Razon**: `aptos-crypto` depende de `merlin` (fork de Aptos) para transcripts criptograficos.

**En Cargo.toml raiz**:
```toml
[patch.crates-io]
merlin = { git = "https://github.com/aptos-labs/merlin" }
```

**RIESGO**: Este patch reemplaza `merlin` para TODO el workspace. Si alguna otra dependencia (e.g., crates de Sui, curve25519-dalek) usa `merlin`, podria causar conflictos o comportamiento inesperado. Verificar con:

```bash
cargo tree -i merlin  # Ver quien depende de merlin actualmente
```

### Impacto en Tiempo de Compilacion

Las dependencias de `aptos-core` son pesadas. Estimaciones de impacto:

- **Primera compilacion**: +3-5 minutos (descarga git + compilacion de ~100 crates transitivos)
- **Compilacion incremental**: +10-20 segundos (si solo cambia src/chain/aptos.rs)
- **Tamano de target/**: +500MB-1GB adicionales
- **Docker build**: +2-4 minutos en fast-build.sh

### Compatibilidad con Dependencias Existentes

Puntos de conflicto potencial:

| Dependencia Existente | Posible Conflicto | Severidad |
|---|---|---|
| `tokio 1.49` | Aptos espera tokio >= 1.35, OK | Ninguna |
| `bcs 0.1` (Sui) | Sui usa bcs de MystenLabs via git, Aptos de crates.io | **Alta** - posible conflicto de nombres |
| `ed25519-dalek 2.1` | Aptos trae su propia version de ed25519 | **Media** - verificar compatibilidad |
| `reqwest 0.12` | Aptos rest-client depende de reqwest | **Baja** - probablemente compatible |
| `serde 1.0` | Compartida, sin conflicto | Ninguna |

**El conflicto de `bcs` es el mas preocupante**. Si Sui y Aptos traen versiones incompatibles del crate `bcs`, podrian necesitarse renaming o features excluyentes.

---

## 9. Evaluacion de Riesgo

### Riesgo Alto

1. **Conflicto de dependencias `bcs`**: Sui usa `bcs` de MystenLabs (via git), Aptos usa `bcs` de crates.io. Si son crates diferentes con el mismo nombre, causara error de compilacion. **Mitigacion**: Compilar con ambos features habilitados (`--features sui,aptos`) en un branch de prueba antes de integrar.

2. **Patch `merlin` global**: El patch `[patch.crates-io] merlin = ...` afecta a TODO el workspace. Si otras dependencias (curve25519-dalek, ed25519-dalek para NEAR o Stellar) dependen de `merlin` original, podria romper esas cadenas. **Mitigacion**: Ejecutar `cargo tree -i merlin` y verificar compatibilidad antes de aplicar.

3. **Arbol de dependencias masivo de aptos-core**: El repositorio aptos-core es enorme (~500+ crates). Traer 4 crates de ahi arrastra cientos de dependencias transitivas, aumentando drasticamente el tiempo de compilacion y el tamano del binario. **Mitigacion**: Usar feature gates estrictos (`#[cfg(feature = "aptos")]`) para que la compilacion base no se vea afectada.

4. **Deserializacion BCS fragil**: La logica de `deserialize_aptos_transaction` usa un hack para detectar fee payer transactions (verificar si los ultimos 33 bytes tienen tag `1`). Esto es fragil y podria romperse si Aptos cambia la estructura de serializacion. **Mitigacion**: Tests exhaustivos con transacciones reales de mainnet y testnet.

### Riesgo Medio

5. **Address USDC en testnet no verificada**: Upstream usa la misma address de mainnet como placeholder para testnet. Esto probablemente es incorrecto. **Mitigacion**: Verificar address real de USDC testnet en Circle docs o explorador de Aptos.

6. **Solo Ed25519 para non-sponsored**: La implementacion actual solo soporta Ed25519 para transacciones no-sponsoreadas. Si un usuario tiene una cuenta Secp256k1, el settlement fallara. **Mitigacion**: La gran mayoria de cuentas Aptos son Ed25519, riesgo aceptable para v1.

7. **Tiempo de compilacion Docker**: Las dependencias de aptos-core podrian hacer que `fast-build.sh` tome 5+ minutos en vez de ~35 segundos. **Mitigacion**: El cache de Cargo mitiga compilaciones incrementales; solo la primera compilacion sera lenta.

8. **Rust Edition 2021 vs aptos-core**: Nuestro fork usa edition 2021, pero aptos-core podria requerir features de edition 2024. **Mitigacion**: El tag `aptos-node-v1.39.2` deberia ser compatible con edition 2021, pero verificar.

### Riesgo Bajo

9. **RPC publico de Aptos**: Las URLs publicas (`fullnode.mainnet.aptoslabs.com`) tienen rate limits. Para produccion con alto volumen, sera necesario un proveedor premium. **Mitigacion**: Comenzar con endpoints publicos, migrar a premium si hay problemas.

10. **Logo de Aptos**: Necesidad de obtener y procesar el logo. **Mitigacion**: Tarea menor, logos oficiales disponibles publicamente.

---

## 10. Estimacion de Esfuerzo

### Desglose por Tarea

| Tarea | Horas | Complejidad | Notas |
|---|---|---|---|
| **1. Patches (aptos-runtimes, merlin)** | 2-3h | Baja | Copiar de upstream, verificar compilacion |
| **2. Cargo.toml (dependencias + features)** | 3-4h | Alta | Resolver conflictos de dependencias |
| **3. Verificar compilacion con features** | 4-6h | Alta | Resolver conflictos bcs/merlin/ed25519 |
| **4. src/chain/aptos.rs (AptosProvider)** | 8-12h | Alta | Portar provider + facilitator de upstream |
| **5. src/network.rs (enum + USDC)** | 2-3h | Media | Seguir patron de Sui |
| **6. src/from_env.rs (env vars)** | 1-2h | Baja | Agregar constantes |
| **7. src/chain/mod.rs (integracion)** | 2-3h | Media | Nuevo variant en enum |
| **8. src/caip2.rs (namespace)** | 1-2h | Baja | Agregar Aptos namespace |
| **9. src/types.rs (payload types)** | 2-3h | Media | ExactAptosPayload, MixedAddress |
| **10. src/handlers.rs (logo route)** | 1h | Baja | Ruta de imagen |
| **11. Frontend (index.html + logo)** | 2-3h | Baja | Tarjeta de red + traducciones |
| **12. .env.example + documentacion** | 1-2h | Baja | Variables de entorno |
| **13. AWS config (Secrets Manager + Terraform)** | 3-4h | Media | Crear secretos, actualizar task def |
| **14. Wallet setup (mainnet + testnet)** | 2-3h | Media | Generar wallets, fondear APT |
| **15. Testing end-to-end** | 4-6h | Alta | Verificar pagos reales en testnet |
| **16. Docker build + deploy** | 2-3h | Media | Compilar con feature aptos |
| **TOTAL** | **38-58h** | | **5-7 dias laborales** |

### Fases Sugeridas

**Fase 1 - Fundacion (Dia 1-2)**:
- Crear patches/aptos-runtimes/
- Agregar dependencias a Cargo.toml
- Resolver conflictos de compilacion (bcs, merlin)
- Verificar que `cargo build --features aptos` compila sin errores

**Fase 2 - Implementacion Core (Dia 3-4)**:
- Crear `src/chain/aptos.rs` con AptosProvider
- Modificar `src/network.rs`, `src/chain/mod.rs`, `src/from_env.rs`
- Modificar `src/caip2.rs`, `src/types.rs`
- Verificar compilacion completa

**Fase 3 - Integracion y Frontend (Dia 5)**:
- Agregar logo y tarjeta en `static/index.html`
- Agregar ruta en `src/handlers.rs`
- Actualizar `.env.example`
- Test local con `cargo run --features aptos`

**Fase 4 - AWS y Deploy (Dia 6)**:
- Generar wallets Aptos (mainnet + testnet)
- Configurar Secrets Manager
- Actualizar Terraform task definition
- Build Docker con `./scripts/fast-build.sh`

**Fase 5 - Testing y Verificacion (Dia 7)**:
- Pruebas en testnet
- Verificar /supported endpoint
- Verificar pagos reales
- Actualizar README.md y stablecoin matrix

---

## 11. Checklist de Verificacion

### Compilacion

- [ ] `patches/aptos-runtimes/` creado y funcional
- [ ] `[patch.crates-io] merlin` no rompe otras dependencias
- [ ] `cargo build --release --features aptos` compila sin errores
- [ ] `cargo build --release --features sui,aptos` compila sin conflictos de bcs
- [ ] `cargo build --release` (sin feature aptos) sigue compilando normalmente
- [ ] `cargo clippy --features aptos` no reporta warnings nuevos
- [ ] `cargo fmt --check` pasa

### Funcionalidad

- [ ] `Network::Aptos` y `Network::AptosTestnet` definidos en network.rs
- [ ] `NetworkFamily::Aptos` definido y mapeado correctamente
- [ ] `Namespace::Aptos` definido en caip2.rs
- [ ] CAIP-2 parsing: `"aptos:1"` -> `Network::Aptos`, `"aptos:2"` -> `Network::AptosTestnet`
- [ ] v1 parsing: `"aptos"` -> `Network::Aptos`, `"aptos-testnet"` -> `Network::AptosTestnet`
- [ ] `ExactAptosPayload` definido en types.rs
- [ ] `MixedAddress::Aptos` variant definido
- [ ] `AptosProvider::from_env()` lee variables de entorno correctamente
- [ ] `AptosProvider::verify()` valida transaccion BCS correctamente
- [ ] `AptosProvider::settle()` firma como fee payer y envia transaccion
- [ ] `AptosProvider::supported()` retorna kinds con `extra.sponsored = true`

### Endpoints

- [ ] `GET /supported` incluye `aptos` y `aptos-testnet`
- [ ] `POST /verify` acepta payloads Aptos (scheme=exact, network=aptos:1)
- [ ] `POST /settle` liquida pagos Aptos correctamente
- [ ] `GET /aptos.png` sirve logo
- [ ] `GET /` muestra tarjeta de red Aptos en landing page

### Configuracion

- [ ] `.env.example` incluye `RPC_URL_APTOS`, `RPC_URL_APTOS_TESTNET`
- [ ] `.env.example` incluye `APTOS_PRIVATE_KEY_MAINNET`, `APTOS_PRIVATE_KEY_TESTNET`
- [ ] Variables de entorno mapeadas en `from_env.rs`
- [ ] RPC URLs publicas funcionan: `https://fullnode.mainnet.aptoslabs.com/v1`

### AWS y Produccion

- [ ] Secreto `facilitator-aptos-private-key-mainnet` creado en Secrets Manager
- [ ] Secreto `facilitator-aptos-private-key-testnet` creado en Secrets Manager
- [ ] RPC URLs de Aptos agregadas al secreto `facilitator-rpc-mainnet`
- [ ] Task definition de Terraform actualizada con env vars y secretos
- [ ] Wallet mainnet fondeada con >= 5 APT
- [ ] Wallet testnet fondeada con APT de faucet
- [ ] Permisos IAM actualizados para nuevos secretos
- [ ] Docker image compilada con `--features aptos`
- [ ] ECS service actualizado con nueva imagen

### Documentacion

- [ ] README.md actualizado con conteo de redes
- [ ] `python scripts/stablecoin_matrix.py` actualizado para incluir Aptos
- [ ] stablecoin_matrix.py script parseando USDC_APTOS correctamente
- [ ] CHANGELOG.md entry para nueva version
- [ ] guides/ADDING_NEW_CHAINS.md menciona Aptos como ejemplo no-EVM

### Testing

- [ ] Test local: `cargo run --features aptos` + `curl /supported | jq '.kinds[] | select(.network | contains("aptos"))'`
- [ ] Test de verificacion: Enviar payload Aptos valido a `/verify`
- [ ] Test de settlement: Enviar payload Aptos valido a `/settle` en testnet
- [ ] Test de rechazo: Payload con amount incorrecto rechazado
- [ ] Test de rechazo: Payload con recipient incorrecto rechazado
- [ ] Test de rechazo: Payload con asset incorrecto rechazado
- [ ] Test de firma: Transaccion con firma invalida rechazada
- [ ] Test Docker: Imagen con feature aptos funciona correctamente

---

## Apendice A: Comparacion con Implementacion Sui

La implementacion de Aptos sigue un patron similar a Sui pero con diferencias clave:

| Aspecto | Sui | Aptos |
|---|---|---|
| **Lenguaje** | Move (Sui variant) | Move (Aptos variant) |
| **Sponsored Txs** | Si (GasData con sponsor) | Si (Fee Payer nativo) |
| **Serializacion** | BCS (via sui-types) | BCS (via aptos-types) |
| **Firma** | Ed25519/Secp256k1/Secp256r1 | Ed25519 (principal) |
| **RPC** | JSON-RPC (WebSocket) | REST API (HTTP) |
| **Address** | 32 bytes hex | 32 bytes hex |
| **USDC format** | Coin type string | Metadata address |
| **Chain ID** | Hash del genesis | Numero simple (1, 2) |
| **Feature gate** | `#[cfg(feature = "sui")]` | `#[cfg(feature = "aptos")]` |
| **Dependencies** | MystenLabs/sui (git) | aptos-labs/aptos-core (git) |
| **Patches needed** | Ninguno | 2 (aptos-runtimes, merlin) |
| **Compile time impact** | +2-3 min | +3-5 min (mas pesado) |
| **Lineas de codigo** | ~609 (sui.rs) | ~500-600 (estimado) |

## Apendice B: Comando de Verificacion Rapida Post-Deploy

```bash
# Verificar que Aptos aparece en /supported
curl -s https://facilitator.ultravioletadao.xyz/supported | \
  jq '.kinds[] | select(.network | contains("aptos"))'

# Verificar CAIP-2 format
curl -s https://facilitator.ultravioletadao.xyz/supported | \
  jq '.kinds[] | select(.network == "aptos:1" or .network == "aptos")'

# Verificar que el signer esta configurado
curl -s https://facilitator.ultravioletadao.xyz/supported | \
  jq '.signers'

# Verificar health
curl -s https://facilitator.ultravioletadao.xyz/health

# Verificar version
curl -s https://facilitator.ultravioletadao.xyz/version
```

## Apendice C: Referencia de RPC URLs Publicas de Aptos

| Red | URL | Rate Limit |
|---|---|---|
| Mainnet (fullnode) | `https://fullnode.mainnet.aptoslabs.com/v1` | Limitado |
| Testnet (fullnode) | `https://fullnode.testnet.aptoslabs.com/v1` | Limitado |
| Mainnet (indexer) | `https://indexer.mainnet.aptoslabs.com/v1/graphql` | N/A (no necesario) |

Para produccion con alto volumen, considerar:
- **Alchemy**: `https://aptos-mainnet.g.alchemy.com/v2/{API_KEY}`
- **Nodereal**: `https://aptos-mainnet.nodereal.io/v1/{API_KEY}`
- **Ankr**: `https://rpc.ankr.com/aptos/{API_KEY}`

---

*Documento generado el 2026-02-13. Basado en analisis de upstream/main (x402-rs/x402-rs) y nuestro fork (UltravioletaDAO/x402-rs v1.33.3).*
