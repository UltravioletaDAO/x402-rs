# 06 - Extension erc20ApprovalGasSponsoring: Flujo Gasless de Aprobacion ERC-20

**Fecha**: 2026-02-12
**Estado**: PENDIENTE DE IMPLEMENTACION - Feature nueva de upstream
**Esfuerzo estimado**: ~800-1100 lineas de codigo + 1 ABI + tests de integracion
**Riesgo**: ALTO - Involucra envio de gas nativo a wallets de terceros + atomicidad critica
**Version actual del fork**: v1.32.1+
**Version upstream analizada**: v1.1.3 (rama `upstream/main`)
**Spec upstream**: `docs/specs/extensions/erc20_gas_sponsoring.md`

---

## Resumen Ejecutivo

La extension `erc20ApprovalGasSponsoring` permite pagos gasless con **cualquier token ERC-20**, incluso aquellos que NO soportan EIP-3009 ni EIP-2612. Esto es transformacional para la cobertura de stablecoins del facilitador, ya que actualmente estamos limitados a tokens que implementan `transferWithAuthorization` (EIP-3009).

### El problema actual

Nuestro facilitador opera exclusivamente con tokens EIP-3009: USDC, EURC, AUSD, PYUSD, USDT0. Si un token no implementa `transferWithAuthorization()`, simplemente no podemos procesarlo. Esto excluye una cantidad enorme de stablecoins y tokens ERC-20 del ecosistema.

### La solucion

El cliente firma una transaccion EVM normal de `approve(Permit2, amount)` off-chain. El facilitador:
1. Verifica la transaccion firmada (RLP decode + validacion de campos)
2. Fondea la wallet del cliente con gas nativo si no tiene suficiente
3. Broadcastea la transaccion de aprobacion firmada por el cliente
4. Ejecuta el settlement via `x402Permit2Proxy.settle()`

Todo esto se ejecuta en un **bundle atomico** para prevenir front-running.

### Dependencia critica

Esta extension requiere el contrato `x402Permit2Proxy` deployado en cada chain soportada, mas el contrato canonico `Permit2` de Uniswap. Ambos contratos ya existen en la spec upstream pero **aun no estan deployados por nosotros**.

---

## 1. Flujo del Protocolo Completo

### 1.1 Diagrama de Secuencia

```
Cliente                    Servidor (Resource)           Facilitador
   |                              |                           |
   |--- GET /resource ----------->|                           |
   |<-- 402 Payment Required -----|                           |
   |    {accepts: [{                                          |
   |      scheme: "exact",                                    |
   |      asset: "0xDAI...",                                  |
   |      extra: {assetTransferMethod: "permit2"},            |
   |      extensions: {                                       |
   |        erc20ApprovalGasSponsoring: {                     |
   |          info: {version: "1"},                           |
   |          schema: {...}                                   |
   |        }                                                 |
   |      }                                                   |
   |    }]}                                                   |
   |                              |                           |
   | [1] Cliente construye tx approve(Permit2, amount)        |
   | [2] Cliente firma la tx con su private key               |
   | [3] Cliente firma permitWitnessTransferFrom (Permit2)    |
   |                              |                           |
   |--- GET /resource ----------->|                           |
   |    PAYMENT-SIGNATURE: {      |                           |
   |      payload: {              |                           |
   |        signature: "0x...",   |                           |
   |        permit2Authorization: {...}                       |
   |      },                      |                           |
   |      extensions: {           |                           |
   |        erc20ApprovalGasSponsoring: {                     |
   |          info: {             |                           |
   |            from: "0x...",    |                           |
   |            signedTransaction: "0x...",                   |
   |            spender: "0xPermit2",                         |
   |            amount: "MAX_UINT",                           |
   |            version: "1"      |                           |
   |          }                   |                           |
   |        }                     |                           |
   |      }                       |                           |
   |    }                         |                           |
   |                              |--- POST /settle --------->|
   |                              |                           |
   |                              |    [A] Decode RLP de      |
   |                              |        signedTransaction  |
   |                              |    [B] Validar campos     |
   |                              |    [C] Check balance gas  |
   |                              |    [D] Simular bundle     |
   |                              |                           |
   |                              |    === TX ATOMICA ===     |
   |                              |    [1] Fund gas al client |
   |                              |    [2] Broadcast approve  |
   |                              |    [3] Permit2Proxy.settle|
   |                              |    =====================  |
   |                              |                           |
   |                              |<-- 200 {txHash} ---------|
   |<-- 200 /resource ------------|                           |
```

### 1.2 Detalle de Cada Paso

**Paso 1 - Anuncio de capacidad (PaymentRequired):**
El servidor resource incluye en su respuesta 402 la extension `erc20ApprovalGasSponsoring` dentro del campo `extensions`. Esto le indica al cliente SDK que el facilitador puede patrocinar el gas para la aprobacion.

**Paso 2 - Construccion del cliente:**
El cliente SDK debe:
- Consultar `eth_getTransactionCount` para obtener el nonce actual
- Consultar `eth_gasPrice` o `eth_feeHistory` para obtener fees actuales
- Construir la transaccion: `to=token_contract, data=approve(Permit2, MAX_UINT256)`
- Firmar la transaccion con la clave privada del cliente
- Firmar por separado el `permitWitnessTransferFrom` de Permit2

**Paso 3 - Envio al facilitador:**
El payload incluye tanto la firma Permit2 (para settlement) como la `signedTransaction` raw (para la aprobacion sponsoreada).

**Paso 4 - Verificacion del facilitador (detallada en seccion 3).**

**Paso 5 - Settlement atomico (detallada en seccion 4).**

---

## 2. Estado Actual - Como Manejamos Tokens Hoy

### 2.1 Mecanismo EIP-3009 Exclusivo

Nuestro facilitador en `/mnt/z/ultravioleta/dao/x402-rs/src/chain/evm.rs` opera exclusivamente via `transferWithAuthorization` (EIP-3009). El flujo actual es:

```rust
// src/chain/evm.rs:750-930 (simplificado)
async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
    let (contract, payment, eip712_domain) =
        assert_valid_payment(self.inner(), self.chain(), payload, requirements).await?;

    let signed_message = SignedMessage::extract(&payment, &eip712_domain)?;
    match signed_message.signature {
        StructuredSignature::EIP6492 { factory, factory_calldata, inner, .. } => {
            // Multicall3: deploy wallet + transferWithAuthorization
        }
        StructuredSignature::EIP1271(sig) => {
            // Directo: transferWithAuthorization
        }
    }
}
```

### 2.2 Tokens soportados actualmente (todos EIP-3009)

| Token | Redes | Nota |
|-------|-------|------|
| USDC | 20+ redes | EIP-3009 nativo en FiatTokenV2 |
| EURC | Ethereum, Base, Avalanche | EIP-3009 nativo |
| AUSD | Ethereum, Polygon, Arbitrum, Avalanche, Monad, BSC | EIP-3009 nativo |
| PYUSD | Ethereum | EIP-3009 con variante v,r,s |
| USDT0 | Arbitrum, Celo, Optimism, Monad | LayerZero OFT con EIP-3009 |

### 2.3 Tokens que NO podemos procesar (sin EIP-3009)

| Token | Capitaliz. aprox. | Por que no funciona |
|-------|-------------------|---------------------|
| DAI | $5B+ | No implementa `transferWithAuthorization` |
| USDT (legacy) | $140B+ | Ethereum USDT no tiene EIP-3009 |
| FRAX | $800M+ | Sin EIP-3009 |
| LUSD | $200M+ | Sin EIP-3009 |
| cUSD (Celo native) | $50M+ | Sin EIP-3009 |
| GHO (Aave) | $180M+ | Sin EIP-3009 |
| DOLA | $100M+ | Sin EIP-3009 |
| RAI | $30M+ | Sin EIP-3009 |
| sUSD (Synthetix) | $50M+ | Sin EIP-3009 |
| Tokens LP de Uniswap | Variable | ERC-20 estandar |

La extension `erc20ApprovalGasSponsoring` abriria el facilitador a **todos estos tokens** y a cualquier ERC-20 futuro.

---

## 3. Logica de Verificacion - Detalle de Implementacion

### 3.1 Decodificacion RLP de la Transaccion Firmada

El facilitador recibe un `signedTransaction` como hex string que contiene una transaccion EVM RLP-encoded y firmada. Debe decodificarla para validar cada campo.

**Estructura propuesta en Rust:**

```rust
// src/extensions/erc20_gas_sponsoring.rs (nuevo archivo)

use alloy::consensus::{TxEnvelope, Transaction as ConsensusTx};
use alloy::primitives::{Address, Bytes, U256};
use alloy::rlp::Decodable;

/// Datos de la extension erc20ApprovalGasSponsoring extraidos del payload.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Erc20ApprovalGasSponsoringData {
    /// Direccion del remitente (cliente).
    pub from: Address,
    /// Direccion del contrato ERC-20 a aprobar.
    pub asset: Address,
    /// Direccion del spender (debe ser Permit2 canonico).
    pub spender: Address,
    /// Monto de aprobacion (tipicamente MaxUint256).
    pub amount: String,
    /// Transaccion firmada RLP-encoded como hex string.
    pub signed_transaction: String,
    /// Version del schema.
    pub version: String,
}

/// Resultado de decodificar y validar la transaccion firmada.
#[derive(Debug)]
pub struct DecodedApprovalTx {
    /// Direccion recuperada del firmante.
    pub signer: Address,
    /// Direccion destino (debe ser el contrato del token).
    pub to: Address,
    /// Calldata (debe ser approve(spender, amount)).
    pub calldata: Bytes,
    /// Nonce on-chain del firmante.
    pub nonce: u64,
    /// Max fee per gas (EIP-1559) o gas price (legacy).
    pub max_fee: u128,
    /// Max priority fee per gas (EIP-1559, 0 para legacy).
    pub max_priority_fee: u128,
    /// Gas limit.
    pub gas_limit: u64,
    /// Chain ID.
    pub chain_id: u64,
    /// Transaccion raw original para broadcast.
    pub raw_tx: Bytes,
}

/// Decodifica una transaccion firmada desde hex RLP-encoded.
///
/// Realiza la decodificacion RLP y recupera la direccion del firmante
/// mediante ecrecover del hash de la transaccion.
pub fn decode_signed_approval_tx(
    signed_tx_hex: &str,
) -> Result<DecodedApprovalTx, Erc20GasSponsoringError> {
    // Remover prefijo 0x si existe
    let hex_clean = signed_tx_hex.strip_prefix("0x").unwrap_or(signed_tx_hex);
    let raw_bytes = hex::decode(hex_clean)
        .map_err(|e| Erc20GasSponsoringError::InvalidHex(e.to_string()))?;

    // Decodificar envelope RLP (soporta legacy, EIP-2930, EIP-1559, EIP-4844)
    let tx_envelope = TxEnvelope::decode(&mut raw_bytes.as_slice())
        .map_err(|e| Erc20GasSponsoringError::RlpDecode(e.to_string()))?;

    // Recuperar firmante via ecrecover
    let signer = tx_envelope.recover_signer()
        .map_err(|e| Erc20GasSponsoringError::SignerRecovery(e.to_string()))?;

    // Extraer campos comunes de la transaccion
    let tx = tx_envelope.as_transaction();
    let to = tx.to().ok_or(Erc20GasSponsoringError::ContractCreation)?;
    let calldata = tx.input().clone();
    let nonce = tx.nonce();
    let gas_limit = tx.gas_limit();
    let chain_id = tx.chain_id()
        .ok_or(Erc20GasSponsoringError::MissingChainId)?;

    // Extraer fees segun tipo de transaccion
    let (max_fee, max_priority_fee) = match &tx_envelope {
        TxEnvelope::Eip1559(signed) => {
            let inner = signed.tx();
            (inner.max_fee_per_gas, inner.max_priority_fee_per_gas)
        }
        TxEnvelope::Legacy(signed) => {
            let inner = signed.tx();
            (inner.gas_price, 0)
        }
        TxEnvelope::Eip2930(signed) => {
            let inner = signed.tx();
            (inner.gas_price, 0)
        }
        _ => return Err(Erc20GasSponsoringError::UnsupportedTxType),
    };

    Ok(DecodedApprovalTx {
        signer,
        to,
        calldata,
        nonce,
        max_fee,
        max_priority_fee,
        gas_limit,
        chain_id,
        raw_tx: raw_bytes.into(),
    })
}
```

### 3.2 Validacion de Campos

Una vez decodificada la transaccion, el facilitador DEBE verificar:

```rust
/// Selector de la funcion approve(address,uint256) en ERC-20.
/// keccak256("approve(address,uint256)") = 0x095ea7b3...
const APPROVE_SELECTOR: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];

/// Direccion canonica del contrato Permit2 de Uniswap.
/// Misma en todas las chains EVM soportadas.
const CANONICAL_PERMIT2: Address = address!("0x000000000022D473030F116dDEE9F6B43aC78BA3");

/// Valida todos los campos de la transaccion de aprobacion decodificada.
pub async fn validate_approval_tx<P: Provider>(
    provider: &P,
    decoded: &DecodedApprovalTx,
    extension_data: &Erc20ApprovalGasSponsoringData,
    chain: &EvmChain,
) -> Result<GasFundingRequirement, Erc20GasSponsoringError> {
    // 1. Verificar que el firmante coincide con `from` en la extension
    if decoded.signer != extension_data.from {
        return Err(Erc20GasSponsoringError::SignerMismatch {
            expected: extension_data.from,
            recovered: decoded.signer,
        });
    }

    // 2. Verificar que `to` es el contrato del asset (token ERC-20)
    if decoded.to != extension_data.asset {
        return Err(Erc20GasSponsoringError::WrongTarget {
            expected: extension_data.asset,
            actual: decoded.to,
        });
    }

    // 3. Verificar que el calldata es approve(Permit2, amount)
    validate_approve_calldata(
        &decoded.calldata,
        &extension_data.spender,
        &extension_data.amount,
    )?;

    // 4. Verificar que el spender es Permit2 canonico
    if extension_data.spender != CANONICAL_PERMIT2 {
        return Err(Erc20GasSponsoringError::InvalidSpender {
            expected: CANONICAL_PERMIT2,
            actual: extension_data.spender,
        });
    }

    // 5. Verificar chain ID
    if decoded.chain_id != chain.chain_id {
        return Err(Erc20GasSponsoringError::ChainIdMismatch {
            expected: chain.chain_id,
            actual: decoded.chain_id,
        });
    }

    // 6. Verificar nonce actual del usuario on-chain
    let on_chain_nonce = provider
        .get_transaction_count(extension_data.from)
        .await
        .map_err(|e| Erc20GasSponsoringError::RpcError(e.to_string()))?;

    if decoded.nonce != on_chain_nonce {
        return Err(Erc20GasSponsoringError::NonceMismatch {
            expected: on_chain_nonce,
            actual: decoded.nonce,
        });
    }

    // 7. Verificar fees razonables (no excesivos, no insuficientes)
    validate_gas_fees(provider, decoded).await?;

    // 8. Calcular deficit de gas del usuario
    let user_balance = provider
        .get_balance(extension_data.from)
        .await
        .map_err(|e| Erc20GasSponsoringError::RpcError(e.to_string()))?;

    let required_gas = U256::from(decoded.gas_limit) * U256::from(decoded.max_fee);
    let gas_deficit = if user_balance >= required_gas {
        U256::ZERO // El usuario ya tiene suficiente gas
    } else {
        required_gas - user_balance
    };

    Ok(GasFundingRequirement {
        needs_funding: gas_deficit > U256::ZERO,
        deficit: gas_deficit,
        user_balance,
        required_gas,
    })
}

/// Valida que el calldata sea exactamente approve(spender, amount).
fn validate_approve_calldata(
    calldata: &Bytes,
    expected_spender: &Address,
    expected_amount: &str,
) -> Result<(), Erc20GasSponsoringError> {
    // Calldata minimo: 4 bytes selector + 32 bytes address + 32 bytes uint256 = 68 bytes
    if calldata.len() < 68 {
        return Err(Erc20GasSponsoringError::InvalidCalldata(
            "Calldata demasiado corto para approve(address,uint256)".into(),
        ));
    }

    // Verificar selector de funcion
    if calldata[..4] != APPROVE_SELECTOR {
        return Err(Erc20GasSponsoringError::InvalidCalldata(
            format!(
                "Selector incorrecto: esperado 0x095ea7b3, recibido 0x{}",
                hex::encode(&calldata[..4])
            ),
        ));
    }

    // Decodificar parametros ABI (address esta en bytes 16..36 del primer word)
    let spender_bytes = &calldata[16..36]; // 4 + 12 padding + 20 address bytes
    let decoded_spender = Address::from_slice(spender_bytes);

    if decoded_spender != *expected_spender {
        return Err(Erc20GasSponsoringError::InvalidCalldata(
            format!(
                "Spender incorrecto: esperado {}, decodificado {}",
                expected_spender, decoded_spender
            ),
        ));
    }

    // Decodificar amount (bytes 36..68)
    let amount_bytes = &calldata[36..68];
    let decoded_amount = U256::from_be_slice(amount_bytes);
    let expected_amount_u256 = U256::from_str_radix(expected_amount, 10)
        .map_err(|e| Erc20GasSponsoringError::InvalidCalldata(
            format!("Amount no parseable: {}", e),
        ))?;

    if decoded_amount != expected_amount_u256 {
        return Err(Erc20GasSponsoringError::InvalidCalldata(
            format!(
                "Amount incorrecto: esperado {}, decodificado {}",
                expected_amount_u256, decoded_amount
            ),
        ));
    }

    Ok(())
}

/// Resultado del analisis de requerimiento de fondeo de gas.
#[derive(Debug)]
pub struct GasFundingRequirement {
    /// Si el usuario necesita que le fondeen gas.
    pub needs_funding: bool,
    /// Deficit de gas nativo (0 si no necesita fondeo).
    pub deficit: U256,
    /// Balance actual de gas nativo del usuario.
    pub user_balance: U256,
    /// Gas total requerido para la transaccion de aprobacion.
    pub required_gas: U256,
}
```

### 3.3 Validacion de Fees de Gas

```rust
/// Valida que los fees de gas en la transaccion firmada sean razonables.
///
/// Verifica:
/// - maxFee no sea excesivamente alto (>5x el precio actual de gas)
/// - maxFee no sea tan bajo que la transaccion no se mine
/// - Gas limit sea suficiente para approve() (~46,000 gas tipicamente)
async fn validate_gas_fees<P: Provider>(
    provider: &P,
    decoded: &DecodedApprovalTx,
) -> Result<(), Erc20GasSponsoringError> {
    let current_gas_price = provider
        .get_gas_price()
        .await
        .map_err(|e| Erc20GasSponsoringError::RpcError(e.to_string()))?;

    // Verificar que maxFee no sea >5x el precio actual (proteccion contra abuso)
    let max_acceptable = current_gas_price.saturating_mul(5);
    if decoded.max_fee > max_acceptable {
        return Err(Erc20GasSponsoringError::ExcessiveGasFee {
            max_fee: decoded.max_fee,
            current_gas: current_gas_price,
            threshold_multiplier: 5,
        });
    }

    // Verificar que maxFee sea al menos 50% del precio actual (para que se mine)
    let min_acceptable = current_gas_price / 2;
    if decoded.max_fee < min_acceptable {
        return Err(Erc20GasSponsoringError::InsufficientGasFee {
            max_fee: decoded.max_fee,
            current_gas: current_gas_price,
        });
    }

    // Gas limit para approve() tipicamente es ~46,000
    // Aceptar entre 30,000 y 200,000 como rango razonable
    const MIN_APPROVE_GAS: u64 = 30_000;
    const MAX_APPROVE_GAS: u64 = 200_000;

    if decoded.gas_limit < MIN_APPROVE_GAS || decoded.gas_limit > MAX_APPROVE_GAS {
        return Err(Erc20GasSponsoringError::UnreasonableGasLimit {
            gas_limit: decoded.gas_limit,
            min: MIN_APPROVE_GAS,
            max: MAX_APPROVE_GAS,
        });
    }

    Ok(())
}
```

---

## 4. Logica de Settlement - Bundle Atomico

### 4.1 Patron de Ejecucion Atomica

La especificacion upstream requiere que las tres operaciones (fondeo + aprobacion + settlement) se ejecuten **atomicamente**. Nuestro facilitador ya tiene un patron similar en `src/chain/evm.rs` usando `Multicall3.aggregate3()` para deploy + transferWithAuthorization. Sin embargo, el bundle de gas sponsoring es mas complejo porque incluye tres transacciones que NO pueden empaquetarse en un solo Multicall3.

**Problema critico:** Multicall3 ejecuta llamadas internas como `delegatecall` desde la misma direccion. Pero aqui necesitamos:
1. Enviar ETH desde el facilitador a la wallet del cliente (transferencia nativa)
2. Broadcastear una transaccion firmada por el CLIENTE (no por el facilitador)
3. Llamar a `x402Permit2Proxy.settle()` desde el facilitador

Las operaciones 1 y 2 NO pueden estar en el mismo Multicall3 porque la operacion 2 es una transaccion independiente firmada por otra entidad. Esto requiere una estrategia diferente.

### 4.2 Estrategia de Atomicidad: Flashbots/Bundle o Secuencial con Protecciones

Hay tres enfoques posibles:

#### Opcion A: Bundles de Flashbots (Recomendado para chains con soporte)

```rust
/// Ejecuta el settlement atomico usando un bundle de transacciones.
///
/// En chains con Flashbots (Ethereum, Base, Polygon, Arbitrum, Optimism):
/// - Usa `eth_sendBundle` para enviar las 3 transacciones como bundle atomico
/// - Si alguna falla, todas se revierten
///
/// En chains sin Flashbots:
/// - Fallback a ejecucion secuencial con protecciones
pub async fn execute_gas_sponsored_settlement<P: MetaEvmProvider>(
    provider: &P,
    extension_data: &Erc20ApprovalGasSponsoringData,
    decoded_approval: &DecodedApprovalTx,
    permit2_settlement: &Permit2SettlementParams,
    gas_funding: &GasFundingRequirement,
) -> Result<SettleResponse, Erc20GasSponsoringError> {
    if supports_bundles(provider.chain()) {
        execute_bundled_settlement(
            provider, extension_data, decoded_approval,
            permit2_settlement, gas_funding,
        ).await
    } else {
        execute_sequential_settlement(
            provider, extension_data, decoded_approval,
            permit2_settlement, gas_funding,
        ).await
    }
}
```

#### Opcion B: Ejecucion Secuencial con Protecciones Anti-Frontrunning

Para chains sin soporte de bundles:

```rust
/// Ejecucion secuencial del flujo gasless con protecciones.
///
/// Orden de operaciones:
/// 1. Verificar que el allowance actual del usuario a Permit2 sea 0
///    (si ya tiene allowance, saltar al paso 3)
/// 2. Si necesita fondeo: enviar gas nativo + esperar confirmacion
/// 3. Broadcastear la transaccion de aprobacion del cliente
/// 4. Esperar confirmacion de la aprobacion
/// 5. Verificar on-chain que el allowance es correcto
/// 6. Ejecutar settlement via x402Permit2Proxy.settle()
///
/// Protecciones:
/// - Verificacion pre y post del allowance on-chain
/// - Timeout agresivo entre cada paso (5s)
/// - Si el paso 2 o 3 falla, NO se ejecuta el settlement
/// - El deficit de gas se calcula con un buffer de 20% de seguridad
async fn execute_sequential_settlement<P: MetaEvmProvider>(
    provider: &P,
    extension_data: &Erc20ApprovalGasSponsoringData,
    decoded_approval: &DecodedApprovalTx,
    permit2_settlement: &Permit2SettlementParams,
    gas_funding: &GasFundingRequirement,
) -> Result<SettleResponse, Erc20GasSponsoringError> {
    // Paso 1: Verificar allowance actual
    let current_allowance = check_permit2_allowance(
        provider.inner(),
        &extension_data.asset,
        &extension_data.from,
    ).await?;

    if current_allowance >= U256::from_str_radix(&permit2_settlement.amount, 10)? {
        // Ya tiene allowance suficiente, ir directo a settlement
        tracing::info!(
            from = %extension_data.from,
            allowance = %current_allowance,
            "Usuario ya tiene allowance Permit2 suficiente, saltando aprobacion"
        );
        return execute_permit2_settle(provider, permit2_settlement).await;
    }

    // Paso 2: Fondear gas si es necesario
    if gas_funding.needs_funding {
        let funding_amount = gas_funding.deficit
            .saturating_mul(U256::from(120))
            .checked_div(U256::from(100))
            .unwrap_or(gas_funding.deficit); // Buffer 20%

        tracing::info!(
            to = %extension_data.from,
            amount = %funding_amount,
            deficit = %gas_funding.deficit,
            "Fondeando gas nativo al cliente"
        );

        let funding_receipt = provider.send_transaction(MetaTransaction {
            to: extension_data.from,
            calldata: Bytes::new(), // Transferencia nativa, sin calldata
            confirmations: 1,
            value: Some(funding_amount), // NOTA: MetaTransaction necesita campo `value`
        }).await.map_err(|e| Erc20GasSponsoringError::GasFundingFailed(
            format!("Fallo al fondear gas: {e}")
        ))?;

        tracing::info!(
            tx = %funding_receipt.transaction_hash,
            "Gas funding confirmado"
        );
    }

    // Paso 3: Broadcastear la transaccion de aprobacion del cliente
    let approval_tx_hash = provider.inner()
        .send_raw_transaction(&decoded_approval.raw_tx)
        .await
        .map_err(|e| Erc20GasSponsoringError::ApprovalBroadcastFailed(
            format!("Fallo al broadcastear approve: {e}")
        ))?;

    tracing::info!(
        tx = %approval_tx_hash,
        from = %extension_data.from,
        asset = %extension_data.asset,
        "Transaccion de aprobacion broadcasteada"
    );

    // Paso 4: Esperar confirmacion de la aprobacion
    let approval_receipt = wait_for_receipt(
        provider.inner(),
        approval_tx_hash,
        Duration::from_secs(30),
    ).await?;

    if !approval_receipt.status() {
        return Err(Erc20GasSponsoringError::ApprovalReverted {
            tx_hash: format!("{:?}", approval_tx_hash),
        });
    }

    // Paso 5: Verificar allowance on-chain post-aprobacion
    let new_allowance = check_permit2_allowance(
        provider.inner(),
        &extension_data.asset,
        &extension_data.from,
    ).await?;

    if new_allowance < U256::from_str_radix(&permit2_settlement.amount, 10)? {
        return Err(Erc20GasSponsoringError::AllowanceVerificationFailed {
            expected_min: permit2_settlement.amount.clone(),
            actual: new_allowance.to_string(),
        });
    }

    // Paso 6: Ejecutar settlement via Permit2
    execute_permit2_settle(provider, permit2_settlement).await
}
```

#### Opcion C: Multicall3 Parcial + Broadcast Separado

Esta opcion agrupa el fondeo de gas y el settlement en un Multicall3, pero la aprobacion va separada:

```rust
/// Estrategia hibrida:
/// 1. Enviar gas al cliente (si necesario) - transaccion directa
/// 2. Broadcastear aprobacion del cliente - send_raw_transaction
/// 3. Esperar confirmacion
/// 4. Settlement via x402Permit2Proxy.settle() - transaccion directa
///
/// Las operaciones 1 y 4 podrian potencialmente empaquetarse en Multicall3,
/// pero la operacion 2 siempre es independiente (firmada por el cliente).
```

### 4.3 Integracion con x402Permit2Proxy

```rust
/// Parametros para el settlement via Permit2.
#[derive(Debug)]
pub struct Permit2SettlementParams {
    /// Contrato x402Permit2Proxy en esta chain.
    pub proxy_address: Address,
    /// Permit2 PermitTransferFrom struct.
    pub permit: PermitTransferFrom,
    /// Monto a transferir.
    pub amount: String,
    /// Owner (from) de los tokens.
    pub owner: Address,
    /// Witness data (to + validAfter + extra).
    pub witness: Permit2Witness,
    /// Firma del permitWitnessTransferFrom.
    pub signature: Bytes,
}

/// Ejecuta el settlement llamando a x402Permit2Proxy.settle().
async fn execute_permit2_settle<P: MetaEvmProvider>(
    provider: &P,
    params: &Permit2SettlementParams,
) -> Result<SettleResponse, Erc20GasSponsoringError> {
    // Codificar calldata para x402Permit2Proxy.settle()
    let calldata = x402Permit2Proxy::settleCall {
        permit: params.permit.clone(),
        amount: U256::from_str_radix(&params.amount, 10)?,
        owner: params.owner,
        witness: params.witness.clone().into(),
        signature: params.signature.clone(),
    }.abi_encode();

    let receipt = provider.send_transaction(MetaTransaction {
        to: params.proxy_address,
        calldata: calldata.into(),
        confirmations: 1,
    }).await.map_err(|e| Erc20GasSponsoringError::SettlementFailed(
        format!("Permit2 settlement fallo: {e}")
    ))?;

    Ok(SettleResponse {
        success: receipt.status(),
        transaction_hash: Some(TransactionHash::Evm(receipt.transaction_hash.0)),
        ..Default::default()
    })
}
```

---

## 5. Dependencias de Smart Contracts

### 5.1 Contrato Permit2 Canonico (Uniswap)

**Direccion canonica (misma en todas las chains EVM):**
```
0x000000000022D473030F116dDEE9F6B43aC78BA3
```

**Estado de deployment:**

| Chain | Permit2 Deployado | Verificado |
|-------|-------------------|------------|
| Ethereum (1) | Si | etherscan.io |
| Base (8453) | Si | basescan.org |
| Arbitrum (42161) | Si | arbiscan.io |
| Optimism (10) | Si | optimistic.etherscan.io |
| Polygon (137) | Si | polygonscan.com |
| Avalanche (43114) | Si | snowscan.xyz |
| BSC (56) | Si | bscscan.com |
| Celo (42220) | Si | celoscan.io |
| Scroll (534352) | Si | scrollscan.com |
| HyperEVM (999) | VERIFICAR | No confirmado |
| Monad (143) | VERIFICAR | Mainnet reciente |
| Unichain (130) | VERIFICAR | Mainnet reciente |
| Sei (1329) | VERIFICAR | No prioritario |

**NOTA CRITICA**: Antes de habilitar esta extension en una chain, se DEBE verificar que Permit2 esta deployado en esa direccion canonica. Se puede verificar con:

```bash
cast code 0x000000000022D473030F116dDEE9F6B43aC78BA3 --rpc-url <RPC_URL>
```

### 5.2 Contrato x402Permit2Proxy

Este es un contrato NUEVO definido en la spec upstream. Debe ser deployado por nosotros o por el ecosistema x402.

**Funciones clave:**

```solidity
contract x402Permit2Proxy {
    ISignatureTransfer public immutable PERMIT2;

    // Settlement estandar (despues de que el usuario ya tiene allowance)
    function settle(
        ISignatureTransfer.PermitTransferFrom calldata permit,
        uint256 amount,
        address owner,
        Witness calldata witness,
        bytes calldata signature
    ) external;

    // Settlement con EIP-2612 permit integrado (extension eip2612GasSponsoring)
    function settleWith2612(
        EIP2612Permit calldata permit2612,
        uint256 amount,
        ISignatureTransfer.PermitTransferFrom calldata permit,
        address owner,
        Witness calldata witness,
        bytes calldata signature
    ) external;
}
```

**Plan de deployment:**

1. Compilar con `forge build` el contrato de la spec upstream
2. Deployar via CREATE2 (para misma direccion en todas las chains)
3. Verificar en cada block explorer
4. Agregar la direccion como constante en `src/extensions/erc20_gas_sponsoring.rs`

**Costo estimado de deployment por chain:** ~0.001-0.01 ETH/native token (contrato pequeno)

### 5.3 ABI Nuevo Requerido

Se necesita crear `/mnt/z/ultravioleta/dao/x402-rs/abi/x402Permit2Proxy.json` con el ABI del contrato. Tambien se necesita el ABI de Permit2 de Uniswap: `/mnt/z/ultravioleta/dao/x402-rs/abi/Permit2.json`.

```rust
// Nuevos bindings sol! necesarios en el codigo

sol!(
    #[allow(missing_docs)]
    #[derive(Debug)]
    #[sol(rpc)]
    x402Permit2Proxy,
    "abi/x402Permit2Proxy.json"
);

sol!(
    #[allow(missing_docs)]
    #[derive(Debug)]
    #[sol(rpc)]
    Permit2,
    "abi/Permit2.json"
);
```

---

## 6. Estructura de Archivos Propuesta

### 6.1 Archivos nuevos

```
src/
  extensions/                          # NUEVO directorio de modulo
    mod.rs                             # ~30 lineas - re-exports
    erc20_gas_sponsoring.rs            # ~450 lineas - logica principal
    eip2612_gas_sponsoring.rs          # ~200 lineas - variante EIP-2612 (bonus)
    permit2.rs                         # ~150 lineas - interaccion con Permit2/Proxy
    types.rs                           # ~100 lineas - tipos compartidos
    errors.rs                          # ~80 lineas - tipos de error

abi/
  x402Permit2Proxy.json                # ABI del proxy
  Permit2.json                         # ABI de Uniswap Permit2
```

### 6.2 Archivos a modificar

```
src/main.rs                            # +5 lineas - registrar modulo extensions
src/chain/evm.rs                       # +80 lineas - integrar flujo Permit2 en settle()
src/handlers.rs                        # +40 lineas - handler para extension en /settle y /verify
src/types_v2.rs                        # +30 lineas - tipos para extensions
src/network.rs                         # +20 lineas - constantes Permit2/Proxy por chain
src/facilitator_local.rs               # +15 lineas - routing de extension
Cargo.toml                             # Sin cambios (alloy ya incluye rlp)
```

### 6.3 Detalle del modulo `src/extensions/mod.rs`

```rust
//! Soporte para extensiones del protocolo x402.
//!
//! Las extensiones amplian las capacidades del facilitador mas alla
//! del flujo EIP-3009 basico. Cada extension es opt-in y se anuncia
//! en la respuesta 402 Payment Required del servidor.

pub mod erc20_gas_sponsoring;
pub mod permit2;
pub mod types;
pub mod errors;

// Re-exports
pub use erc20_gas_sponsoring::{
    Erc20ApprovalGasSponsoringData,
    decode_signed_approval_tx,
    validate_approval_tx,
    execute_gas_sponsored_settlement,
};
pub use permit2::{
    Permit2SettlementParams,
    execute_permit2_settle,
    check_permit2_allowance,
    CANONICAL_PERMIT2,
};
pub use errors::Erc20GasSponsoringError;
```

### 6.4 Detalle de `src/extensions/errors.rs`

```rust
//! Tipos de error para la extension erc20ApprovalGasSponsoring.

use alloy::primitives::Address;
use thiserror::Error;

use crate::chain::FacilitatorLocalError;

#[derive(Debug, Error)]
pub enum Erc20GasSponsoringError {
    // --- Errores de decodificacion ---
    #[error("Hex invalido en signedTransaction: {0}")]
    InvalidHex(String),

    #[error("Fallo en decodificacion RLP: {0}")]
    RlpDecode(String),

    #[error("No se pudo recuperar el firmante: {0}")]
    SignerRecovery(String),

    #[error("signedTransaction es una creacion de contrato (sin campo 'to')")]
    ContractCreation,

    #[error("Falta chain_id en la transaccion firmada")]
    MissingChainId,

    #[error("Tipo de transaccion no soportado (solo Legacy, EIP-2930, EIP-1559)")]
    UnsupportedTxType,

    // --- Errores de validacion ---
    #[error("Firmante no coincide: esperado {expected}, recuperado {recovered}")]
    SignerMismatch { expected: Address, recovered: Address },

    #[error("Destino incorrecto: esperado {expected} (asset), actual {actual}")]
    WrongTarget { expected: Address, actual: Address },

    #[error("Spender invalido: esperado Permit2 {expected}, actual {actual}")]
    InvalidSpender { expected: Address, actual: Address },

    #[error("Chain ID no coincide: esperado {expected}, actual {actual}")]
    ChainIdMismatch { expected: u64, actual: u64 },

    #[error("Nonce no coincide: on-chain {expected}, en tx {actual}")]
    NonceMismatch { expected: u64, actual: u64 },

    #[error("Calldata invalido: {0}")]
    InvalidCalldata(String),

    #[error("Fee de gas excesivo: maxFee={max_fee}, gas actual={current_gas}, umbral={threshold_multiplier}x")]
    ExcessiveGasFee { max_fee: u128, current_gas: u128, threshold_multiplier: u64 },

    #[error("Fee de gas insuficiente: maxFee={max_fee}, gas actual={current_gas}")]
    InsufficientGasFee { max_fee: u128, current_gas: u128 },

    #[error("Gas limit fuera de rango: {gas_limit} (aceptable: {min}-{max})")]
    UnreasonableGasLimit { gas_limit: u64, min: u64, max: u64 },

    // --- Errores de ejecucion ---
    #[error("Fallo al fondear gas al usuario: {0}")]
    GasFundingFailed(String),

    #[error("Fallo al broadcastear transaccion de aprobacion: {0}")]
    ApprovalBroadcastFailed(String),

    #[error("Transaccion de aprobacion revertida: tx={tx_hash}")]
    ApprovalReverted { tx_hash: String },

    #[error("Verificacion de allowance fallo: esperado min {expected_min}, actual {actual}")]
    AllowanceVerificationFailed { expected_min: String, actual: String },

    #[error("Settlement Permit2 fallo: {0}")]
    SettlementFailed(String),

    #[error("Error RPC: {0}")]
    RpcError(String),

    #[error("Timeout esperando confirmacion de transaccion")]
    TransactionTimeout,

    // --- Errores de configuracion ---
    #[error("Permit2 no deployado en chain {chain_id}")]
    Permit2NotDeployed { chain_id: u64 },

    #[error("x402Permit2Proxy no deployado en chain {chain_id}")]
    ProxyNotDeployed { chain_id: u64 },

    #[error("Extension erc20ApprovalGasSponsoring no soportada en esta chain")]
    ExtensionNotSupported,
}

impl From<Erc20GasSponsoringError> for FacilitatorLocalError {
    fn from(err: Erc20GasSponsoringError) -> Self {
        FacilitatorLocalError::Other(err.to_string())
    }
}
```

### 6.5 Detalle de `src/extensions/permit2.rs`

```rust
//! Interaccion con contratos Permit2 y x402Permit2Proxy.

use alloy::primitives::{address, Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::SolCall;

use super::errors::Erc20GasSponsoringError;
use crate::chain::evm::EvmChain;

/// Direccion canonica del contrato Permit2 de Uniswap.
/// Deployado via CREATE2 en la misma direccion en todas las chains EVM.
/// Ref: https://docs.uniswap.org/contracts/v4/deployments
pub const CANONICAL_PERMIT2: Address =
    address!("0x000000000022D473030F116dDEE9F6B43aC78BA3");

/// Direccion del contrato x402Permit2Proxy.
/// Deployado via CREATE2 para consistencia cross-chain.
/// NOTA: Esta direccion es placeholder - actualizar tras deployment real.
pub const X402_PERMIT2_PROXY: Address =
    address!("0x0000000000000000000000000000000000000000"); // TODO: Direccion real

sol!(
    #[allow(missing_docs)]
    #[derive(Debug)]
    #[sol(rpc)]
    x402Permit2Proxy,
    "abi/x402Permit2Proxy.json"
);

sol!(
    #[allow(missing_docs)]
    #[derive(Debug)]
    #[sol(rpc)]
    Permit2Contract,
    "abi/Permit2.json"
);

/// Devuelve la direccion del x402Permit2Proxy para una chain dada.
///
/// Retorna None si el proxy no esta deployado en esa chain.
pub fn permit2_proxy_for_chain(chain: &EvmChain) -> Option<Address> {
    // El proxy debe estar deployado via CREATE2 en la misma direccion en todas las chains.
    // Verificar que la chain esta en la lista de soportadas.
    match chain.chain_id {
        8453  => Some(X402_PERMIT2_PROXY), // Base
        1     => Some(X402_PERMIT2_PROXY), // Ethereum
        42161 => Some(X402_PERMIT2_PROXY), // Arbitrum
        10    => Some(X402_PERMIT2_PROXY), // Optimism
        137   => Some(X402_PERMIT2_PROXY), // Polygon
        43114 => Some(X402_PERMIT2_PROXY), // Avalanche
        56    => Some(X402_PERMIT2_PROXY), // BSC
        42220 => Some(X402_PERMIT2_PROXY), // Celo
        534352 => Some(X402_PERMIT2_PROXY), // Scroll
        // Testnets
        84532 => Some(X402_PERMIT2_PROXY), // Base Sepolia
        11155111 => Some(X402_PERMIT2_PROXY), // Ethereum Sepolia
        _ => None,
    }
}

/// Verifica si Permit2 esta deployado en una chain consultando el bytecode.
pub async fn verify_permit2_deployed<P: Provider>(
    provider: &P,
) -> Result<bool, Erc20GasSponsoringError> {
    let code = provider
        .get_code_at(CANONICAL_PERMIT2)
        .await
        .map_err(|e| Erc20GasSponsoringError::RpcError(e.to_string()))?;
    Ok(!code.is_empty())
}

/// Consulta el allowance actual de un usuario hacia Permit2 para un token dado.
pub async fn check_permit2_allowance<P: Provider>(
    provider: &P,
    token: &Address,
    owner: &Address,
) -> Result<U256, Erc20GasSponsoringError> {
    // Usar el ABI de ERC-20 standard para consultar allowance(owner, Permit2)
    let usdc_instance = crate::chain::evm::USDC::new(*token, provider);
    let allowance = usdc_instance
        .allowance(*owner, CANONICAL_PERMIT2)
        .call()
        .await
        .map_err(|e| Erc20GasSponsoringError::RpcError(
            format!("Fallo al consultar allowance: {e}")
        ))?;

    Ok(allowance)
}
```

---

## 7. Analisis de Seguridad

### 7.1 Front-running: Riesgo Principal

**Escenario de ataque:**
1. El facilitador fondea gas al cliente (tx1 visible en mempool)
2. Un atacante ve la tx1 y la tx de aprobacion pendientes en el mempool
3. El atacante front-runea con un gas price mas alto para broadcastear una transaccion que drena los fondos del cliente antes de que el facilitador complete el settlement

**Mitigacion:**
- En chains con Flashbots/MEV protection (Base, Ethereum, Arbitrum, Optimism): usar bundles privados que no son visibles en el mempool publico.
- En chains sin MEV protection: la ventana de ataque es minima porque:
  - El approve es para Permit2 (no para un EOA atacante)
  - Solo el x402Permit2Proxy puede usar el allowance de Permit2
  - El witness en Permit2 fija el destinatario (`to`), que no puede ser alterado

**Riesgo residual: MEDIO**. El gas fondeado al cliente podria ser gastado en otra transaccion si el atacante front-runea con el nonce del cliente. Pero la perdida maxima es solo el gas fondeado (~$0.01-0.50 dependiendo de la chain).

### 7.2 Replay de Transaccion de Aprobacion

**Escenario:** Un atacante intercepta la `signedTransaction` y la broadcastea antes que el facilitador.

**Mitigacion:**
- La transaccion tiene un nonce especifico; si se broadcastea exitosamente, el facilitador recibira "nonce too low" y detectara que ya fue procesada.
- El facilitador debe verificar el allowance on-chain DESPUES de que la aprobacion se confirma, no asumir que fue el quien la envio.
- Este escenario en realidad no es un ataque: el resultado deseado (approve a Permit2) se logra de todas formas.

### 7.3 Abuso del Fondeo de Gas

**Escenario:** Un cliente malicioso solicita fondeo de gas repetidamente sin completar settlements.

**Mitigacion:**
- Limitar el monto de gas fondeado al minimo necesario para una sola transaccion de approve (~46k gas * gas_price).
- Implementar rate limiting por direccion `from`.
- Verificar que el usuario realmente tiene tokens ERC-20 suficientes antes de fondear gas.
- Llevar un registro de fondeos y marcar direcciones que abusan.

```rust
/// Maximo gas nativo que el facilitador esta dispuesto a fondear por transaccion.
/// Configurado via variable de entorno MAX_GAS_SPONSORING_WEI.
/// Default: 0.001 ETH (1e15 wei) - suficiente para ~20 approve txs en Base.
pub fn max_gas_sponsoring() -> U256 {
    std::env::var("MAX_GAS_SPONSORING_WEI")
        .ok()
        .and_then(|v| U256::from_str_radix(&v, 10).ok())
        .unwrap_or(U256::from(1_000_000_000_000_000u64)) // 0.001 ETH default
}
```

### 7.4 Verificacion de Calldata Malicioso

**Escenario:** El cliente envia una `signedTransaction` que parece ser `approve()` pero tiene calldata malicioso (ej: `transfer()` disfrazado).

**Mitigacion:** Ya cubierta en la seccion 3.2 - el facilitador DEBE:
1. Decodificar el calldata y verificar el selector de funcion (0x095ea7b3)
2. Verificar que el spender es exactamente Permit2 canonico
3. Verificar que el `to` de la transaccion es el contrato del token declarado
4. Nunca broadcastear una transaccion sin validar completamente su contenido

### 7.5 Recomendacion: Limite de Valor por Extension

```rust
/// Estructura para configuracion de limites de la extension.
pub struct GasSponsoringLimits {
    /// Maximo gas nativo a fondear por transaccion (wei).
    pub max_gas_per_tx: U256,
    /// Maximo de fondeos por direccion por hora.
    pub max_fundings_per_address_per_hour: u32,
    /// Monto minimo de pago para habilitar sponsoring.
    /// No tiene sentido fondear $0.50 de gas para un pago de $0.01.
    pub min_payment_amount: U256,
}

impl Default for GasSponsoringLimits {
    fn default() -> Self {
        Self {
            max_gas_per_tx: U256::from(1_000_000_000_000_000u64), // 0.001 ETH
            max_fundings_per_address_per_hour: 5,
            min_payment_amount: U256::from(100_000u64), // $0.10 USDC (6 decimals)
        }
    }
}
```

---

## 8. Modificaciones a MetaTransaction

Actualmente, `MetaTransaction` en `src/chain/evm.rs:307-314` no soporta envio de valor nativo (ETH/AVAX/MATIC):

```rust
// Estado actual (src/chain/evm.rs:307-314)
pub struct MetaTransaction {
    pub to: Address,
    pub calldata: Bytes,
    pub confirmations: u64,
}
```

**Se necesita agregar un campo `value`:**

```rust
// Propuesta modificada
pub struct MetaTransaction {
    /// Target contract address.
    pub to: Address,
    /// Transaction calldata (encoded function call).
    pub calldata: Bytes,
    /// Number of block confirmations to wait for.
    pub confirmations: u64,
    /// Native value to send (wei). Default: 0.
    /// Usado por erc20ApprovalGasSponsoring para fondear gas.
    pub value: Option<U256>,
}
```

Y la modificacion correspondiente en `send_transaction`:

```rust
// En src/chain/evm.rs, dentro de send_transaction:
let mut txr = TransactionRequest::default()
    .with_to(to)
    .with_from(from_address)
    .with_input(calldata.clone());

// NUEVO: Agregar valor nativo si existe
if let Some(value) = tx.value {
    txr = txr.with_value(value);
}
```

**IMPORTANTE**: Todas las llamadas existentes a `MetaTransaction` deben agregar `value: None` para mantener compatibilidad. Hay aproximadamente 15 instancias en `evm.rs` y varias en `escrow.rs`.

---

## 9. Integracion con el Flujo de Verificacion Existente

### 9.1 Modificacion de `verify()` en EvmProvider

El metodo `verify()` en `src/chain/evm.rs:587` actualmente solo valida EIP-3009. Debe expandirse para detectar y validar la extension:

```rust
// Pseudocodigo de la modificacion a verify()
async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
    let payload = &request.payment_payload;
    let requirements = &request.payment_requirements;

    // NUEVO: Detectar si el payload incluye extension erc20ApprovalGasSponsoring
    if let Some(extension_data) = extract_gas_sponsoring_extension(payload) {
        // Flujo Permit2 con gas sponsoring
        return self.verify_gas_sponsored_payment(
            payload, requirements, &extension_data
        ).await;
    }

    // NUEVO: Detectar si es un pago Permit2 sin gas sponsoring
    if is_permit2_payment(requirements) {
        return self.verify_permit2_payment(payload, requirements).await;
    }

    // Flujo EIP-3009 existente (sin cambios)
    let (contract, payment, eip712_domain) =
        assert_valid_payment(self.inner(), self.chain(), payload, requirements).await?;
    // ... resto del codigo existente
}
```

### 9.2 Modificacion de `settle()` en EvmProvider

```rust
// Pseudocodigo de la modificacion a settle()
async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
    let payload = &request.payment_payload;
    let requirements = &request.payment_requirements;

    // NUEVO: Detectar extension erc20ApprovalGasSponsoring
    if let Some(extension_data) = extract_gas_sponsoring_extension(payload) {
        let decoded_tx = decode_signed_approval_tx(&extension_data.signed_transaction)?;
        let gas_req = validate_approval_tx(
            self.inner(), &decoded_tx, &extension_data, self.chain()
        ).await?;

        let permit2_params = extract_permit2_settlement_params(payload, requirements)?;

        return execute_gas_sponsored_settlement(
            self, &extension_data, &decoded_tx, &permit2_params, &gas_req
        ).await;
    }

    // Flujo EIP-3009 existente (sin cambios)
    let (contract, payment, eip712_domain) =
        assert_valid_payment(self.inner(), self.chain(), payload, requirements).await?;
    // ... resto del codigo existente
}
```

### 9.3 Modificacion de `/supported` Endpoint

El endpoint `/supported` debe anunciar la extension para cada network donde este disponible:

```rust
// En la construccion de SupportedPaymentKindsResponse
SupportedPaymentKind {
    scheme: Scheme::Exact,
    network: network.to_string(),
    asset: token.address.to_string(),
    extra: Some(SupportedPaymentKindExtra {
        // NUEVO: Indicar metodo de transferencia
        asset_transfer_method: Some("permit2".into()),
        ..Default::default()
    }),
    // NUEVO: Incluir extensions soportadas
    extensions: vec!["erc20ApprovalGasSponsoring".into()],
}
```

---

## 10. Dependencias de Crates

### 10.1 Ya incluidas (sin cambios al Cargo.toml)

| Crate | Version | Uso |
|-------|---------|-----|
| `alloy` | 1.0.12 | RLP decoding (`alloy::consensus::TxEnvelope`), `Decodable`, `recover_signer()` |
| `hex` | 0.4 | Hex decode de `signedTransaction` |
| `thiserror` | 2.0.18 | Tipos de error |
| `serde` | 1.0.228 | Deserializacion de extension data |
| `tracing` | 0.1.44 | Logging/instrumentacion |

### 10.2 Verificacion de capacidades de Alloy

La version `alloy 1.0.12` ya incluye todo lo necesario:

```rust
// RLP decoding de transacciones firmadas
use alloy::consensus::TxEnvelope;
use alloy::rlp::Decodable;

// Recuperacion de firmante
// TxEnvelope::recover_signer() disponible en alloy 1.0+

// Tipos de transaccion
use alloy::consensus::transaction::Transaction; // trait comun

// ABI encoding para approve()
use alloy::sol_types::SolCall; // ya en uso
```

**NO se necesitan crates adicionales.** Esta es una ventaja significativa de haber actualizado a `alloy 1.0.12`.

---

## 11. Extension Complementaria: eip2612GasSponsoring

La spec upstream tambien define `eip2612GasSponsoring` como extension complementaria. Es mas simple que `erc20ApprovalGasSponsoring` porque no requiere fondeo de gas ni broadcast de transacciones:

### 11.1 Diferencia clave

| Aspecto | erc20ApprovalGasSponsoring | eip2612GasSponsoring |
|---------|---------------------------|---------------------|
| Requisito del token | Cualquier ERC-20 | Debe implementar EIP-2612 (`permit()`) |
| Fondeo de gas | Si (necesario) | No (gasless nativo) |
| Transacciones del facilitador | 3 (fund + approve + settle) | 1 (`settleWith2612()`) |
| Complejidad | Alta | Media |
| Riesgo | Alto | Bajo |
| Atomicidad | Problematica (3 tx separadas) | Perfecta (1 tx) |

### 11.2 Tokens con EIP-2612 (pero sin EIP-3009)

| Token | Chains | EIP-2612 | Nota |
|-------|--------|----------|------|
| DAI | Ethereum, Arbitrum, Optimism, Polygon | Si | Implementa `permit()` |
| UNI | Ethereum | Si | Governance token de Uniswap |
| AAVE | Ethereum | Si | Governance token de Aave |
| GHO | Ethereum | Si | Stablecoin de Aave |

**Recomendacion:** Implementar `eip2612GasSponsoring` PRIMERO porque es significativamente mas simple y seguro. Luego implementar `erc20ApprovalGasSponsoring` como fallback universal.

### 11.3 Flujo simplificado de eip2612

```
Cliente                              Facilitador
   |                                      |
   | [1] Firma EIP-2612 permit            |
   |     (owner, Permit2, amount,         |
   |      deadline, v, r, s)              |
   |                                      |
   | [2] Firma Permit2 witness            |
   |                                      |
   |--- payload con ambas firmas -------->|
   |                                      |
   |    [3] Verificar firma EIP-2612      |
   |    [4] Verificar firma Permit2       |
   |    [5] Llamar settleWith2612()       |
   |        (1 sola transaccion!)         |
   |                                      |
   |<-- 200 {txHash} --------------------|
```

---

## 12. Estimacion de Esfuerzo

### 12.1 Desglose por componente

| Componente | Lineas nuevas | Lineas modificadas | Complejidad |
|------------|--------------|-------------------|-------------|
| `src/extensions/mod.rs` | 30 | 0 | Baja |
| `src/extensions/erc20_gas_sponsoring.rs` | 450 | 0 | Alta |
| `src/extensions/permit2.rs` | 150 | 0 | Media |
| `src/extensions/types.rs` | 100 | 0 | Baja |
| `src/extensions/errors.rs` | 80 | 0 | Baja |
| `src/chain/evm.rs` (integracion) | 0 | 80 | Alta |
| `src/handlers.rs` (endpoints) | 0 | 40 | Media |
| `src/types_v2.rs` (tipos extension) | 0 | 30 | Baja |
| `src/network.rs` (constantes) | 0 | 20 | Baja |
| `src/facilitator_local.rs` (routing) | 0 | 15 | Media |
| `src/main.rs` (modulo) | 0 | 5 | Baja |
| `MetaTransaction` (campo value) | 0 | 20 | Baja |
| **ABIs** (x402Permit2Proxy.json, Permit2.json) | ~200 | 0 | Baja |
| **Tests unitarios** | 200 | 0 | Media |
| **Tests integracion** | 150 | 0 | Alta |
| **TOTAL** | **~1,160** | **~210** | **Alta** |

### 12.2 Orden de implementacion recomendado

**Fase 1 - Infraestructura (2-3 dias)**
1. Crear `src/extensions/` con mod.rs, types.rs, errors.rs
2. Agregar campo `value` a `MetaTransaction`
3. Agregar ABIs de Permit2 y x402Permit2Proxy
4. Agregar constantes de contrato en network.rs

**Fase 2 - Decodificacion y validacion (2-3 dias)**
5. Implementar `decode_signed_approval_tx()`
6. Implementar `validate_approval_tx()` con todas las verificaciones
7. Implementar `validate_approve_calldata()`
8. Tests unitarios de decodificacion y validacion

**Fase 3 - Settlement (3-4 dias)**
9. Implementar `permit2.rs` con interaccion de contratos
10. Implementar `execute_gas_sponsored_settlement()`
11. Implementar flujo secuencial con protecciones
12. Integrar en `evm.rs` verify() y settle()

**Fase 4 - Integracion y endpoints (1-2 dias)**
13. Modificar handlers.rs para extension
14. Modificar /supported para anunciar extension
15. Actualizar facilitator_local.rs

**Fase 5 - Deployment de contratos y testing (2-3 dias)**
16. Deployar x402Permit2Proxy en Base Sepolia
17. Tests de integracion end-to-end en testnet
18. Deploy en mainnet chains

**Esfuerzo total estimado: 10-15 dias de desarrollo**

### 12.3 Prerrequisitos

- [ ] Contrato x402Permit2Proxy compilado y verificado
- [ ] x402Permit2Proxy deployado en Base Sepolia (testnet)
- [ ] Permit2 verificado como presente en chains objetivo
- [ ] SDK de cliente actualizado para construir payloads con extension
- [ ] Fondos para deployment de contratos (~0.01 ETH por chain)

---

## 13. Evaluacion de Riesgo

### 13.1 Matriz de Riesgos

| Riesgo | Probabilidad | Impacto | Mitigacion |
|--------|-------------|---------|------------|
| Front-running del gas fondeado | Media | Bajo ($0.01-0.50) | Bundles privados, MEV protection |
| Error en estimacion de gas | Media | Medio (tx falla) | Buffer 20%, retry con gas incrementado |
| Nonce race condition | Media | Medio (tx falla) | Verificacion pre/post, retry |
| Contrato x402Permit2Proxy con bug | Baja | Alto (fondos) | Auditar antes de mainnet |
| Permit2 no deployado en chain | Baja | Medio (feature inop) | Check runtime + graceful fallback |
| Abuso repetido de gas sponsoring | Alta | Medio ($$$) | Rate limiting + limites de fondeo |
| Transaccion de approve revertida | Media | Bajo (retry) | Simulacion previa |
| Incompatibilidad con token ERC-20 | Baja | Bajo (feature inop) | Simulacion + fallback |

### 13.2 Riesgo financiero para el facilitador

**Peor caso por transaccion:**
- Gas fondeado: ~0.001 ETH (~$3 en Ethereum, ~$0.001 en Base)
- Si la settlement falla: el facilitador pierde el gas fondeado
- El monto es recuperable si el approve es exitoso (el token queda aprobado para futuro uso)

**Estimacion de perdida maxima por dia:**
- Rate limit: 5 fondeos por direccion por hora
- Maximo 100 direcciones unicas por dia (estimacion conservadora)
- Perdida maxima: 100 * 5 * $0.001 = $0.50/dia en Base
- En Ethereum: 100 * 5 * $3 = $1,500/dia (DEMASIADO - deshabilitar en Ethereum mainnet inicialmente)

### 13.3 Recomendacion de rollout

1. **Fase testnet**: Base Sepolia + Ethereum Sepolia (sin riesgo financiero)
2. **Fase L2**: Base, Arbitrum, Optimism, Polygon (gas barato, riesgo minimo)
3. **Fase L1**: Ethereum mainnet (solo con rate limiting estricto y limites bajos)
4. **Fase expansion**: Avalanche, BSC, Celo, Scroll (post-validacion)

---

## 14. Checklist de Verificacion en Testnet

### 14.1 Pre-deployment

- [ ] Permit2 tiene bytecode en `0x000000000022D473030F116dDEE9F6B43aC78BA3` en Base Sepolia
- [ ] x402Permit2Proxy deployado y verificado en Base Sepolia
- [ ] ABI de x402Permit2Proxy generado y copiado a `abi/`
- [ ] ABI de Permit2 (ISignatureTransfer) copiado a `abi/`

### 14.2 Decodificacion y validacion

- [ ] Decodificar transaccion legacy (Type 0) correctamente
- [ ] Decodificar transaccion EIP-2930 (Type 1) correctamente
- [ ] Decodificar transaccion EIP-1559 (Type 2) correctamente
- [ ] Rechazar transaccion con chain_id incorrecto
- [ ] Rechazar transaccion donde `to` no es el contrato del token
- [ ] Rechazar transaccion con calldata que no es `approve()`
- [ ] Rechazar transaccion con spender que no es Permit2
- [ ] Rechazar transaccion con nonce incorrecto
- [ ] Rechazar transaccion con gas fee excesivo (>5x)
- [ ] Rechazar transaccion con gas fee insuficiente (<50%)
- [ ] Recuperar signer correctamente y comparar con `from`

### 14.3 Settlement completo

- [ ] Usuario SIN gas nativo: fondeo + approve + settle exitoso
- [ ] Usuario CON gas nativo: skip fondeo + approve + settle exitoso
- [ ] Usuario con allowance existente: skip approve + settle exitoso
- [ ] Verificar que el monto correcto llega al `payTo`
- [ ] Verificar que el allowance queda configurado post-settlement
- [ ] Verificar que el hash de transaccion se retorna correctamente

### 14.4 Casos de error

- [ ] Settlement falla si el usuario no tiene tokens suficientes
- [ ] Settlement falla si la transaccion de approve es revertida
- [ ] Gas funding se limita al maximo configurado
- [ ] Rate limiting funciona correctamente
- [ ] Timeout en transaccion pendiente retorna error limpio
- [ ] Nonce conflict (otro tx broadcasteado primero) se maneja gracefully

### 14.5 Integracion con endpoints

- [ ] `POST /verify` acepta payload con extension y retorna validacion
- [ ] `POST /settle` ejecuta flujo completo con extension
- [ ] `GET /supported` anuncia extension para chains soportadas
- [ ] Payloads SIN extension siguen funcionando (EIP-3009 legacy)
- [ ] Payloads con extension en chain no soportada retornan error descriptivo

### 14.6 Test script de integracion

```python
# tests/integration/test_erc20_gas_sponsoring.py

import json
import requests
from web3 import Web3
from eth_account import Account

FACILITATOR_URL = "http://localhost:8080"
TESTNET_RPC = "https://sepolia.base.org"
DAI_BASE_SEPOLIA = "0x..."  # Token sin EIP-3009 en testnet
PERMIT2 = "0x000000000022D473030F116dDEE9F6B43aC78BA3"

def test_full_gas_sponsored_flow():
    """Test completo del flujo erc20ApprovalGasSponsoring."""
    w3 = Web3(Web3.HTTPProvider(TESTNET_RPC))
    account = Account.create()  # Wallet sin gas

    # 1. Construir transaccion de approve
    token = w3.eth.contract(address=DAI_BASE_SEPOLIA, abi=ERC20_ABI)
    approve_tx = token.functions.approve(
        PERMIT2, 2**256 - 1  # MaxUint256
    ).build_transaction({
        'from': account.address,
        'nonce': w3.eth.get_transaction_count(account.address),
        'gasPrice': w3.eth.gas_price,
        'gas': 50000,
        'chainId': 84532,
    })

    # 2. Firmar la transaccion
    signed = account.sign_transaction(approve_tx)

    # 3. Construir Permit2 witness signature (omitido por brevedad)
    # ...

    # 4. Enviar al facilitador
    payload = {
        "x402Version": "2",
        "payload": {
            "signature": "0x...",  # Permit2 witness signature
            "permit2Authorization": { ... },
        },
        "extensions": {
            "erc20ApprovalGasSponsoring": {
                "info": {
                    "from": account.address,
                    "asset": DAI_BASE_SEPOLIA,
                    "spender": PERMIT2,
                    "amount": str(2**256 - 1),
                    "signedTransaction": signed.rawTransaction.hex(),
                    "version": "1"
                }
            }
        }
    }

    # 5. Verify
    resp = requests.post(f"{FACILITATOR_URL}/verify", json={
        "paymentPayload": payload,
        "paymentRequirements": { ... }
    })
    assert resp.status_code == 200
    assert resp.json()["isValid"] == True

    # 6. Settle
    resp = requests.post(f"{FACILITATOR_URL}/settle", json={
        "paymentPayload": payload,
        "paymentRequirements": { ... }
    })
    assert resp.status_code == 200
    result = resp.json()
    assert result["success"] == True
    assert result["transactionHash"] is not None

    print(f"[OK] Settlement exitoso: {result['transactionHash']}")
```

---

## 15. Comparacion con Extension eip2612GasSponsoring

### Implementacion de eip2612 (inclusion recomendada)

La extension `eip2612GasSponsoring` es **significativamente mas simple** porque:
1. NO requiere fondeo de gas
2. NO requiere broadcast de transaccion del cliente
3. Todo se ejecuta en UNA sola transaccion: `x402Permit2Proxy.settleWith2612()`

```rust
// src/extensions/eip2612_gas_sponsoring.rs (~200 lineas)

/// Datos de la extension eip2612GasSponsoring.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Eip2612GasSponsoringData {
    pub from: Address,
    pub asset: Address,
    pub spender: Address,    // Debe ser Permit2 canonico
    pub amount: String,       // Tipicamente MaxUint256
    pub nonce: String,        // Nonce EIP-2612 del token
    pub deadline: String,     // Timestamp de expiracion
    pub signature: String,    // Firma EIP-2612 (65 bytes, r||s||v)
    pub version: String,
}

/// Verificar la extension eip2612.
pub async fn verify_eip2612_extension<P: Provider>(
    provider: &P,
    data: &Eip2612GasSponsoringData,
    chain: &EvmChain,
) -> Result<(), Erc20GasSponsoringError> {
    // 1. Verificar que el asset implementa IERC20Permit
    //    (llamar permit() con datos invalidos y ver si revierte con error esperado)

    // 2. Verificar la firma EIP-2612
    //    (reconstruir EIP-712 domain del token y verificar ecrecover)

    // 3. Verificar que spender es Permit2 canonico

    // 4. Simular settleWith2612() via eth_call
    Ok(())
}

/// Ejecutar settlement con eip2612.
/// Una sola transaccion: x402Permit2Proxy.settleWith2612()
pub async fn execute_eip2612_settlement<P: MetaEvmProvider>(
    provider: &P,
    eip2612_data: &Eip2612GasSponsoringData,
    permit2_params: &Permit2SettlementParams,
) -> Result<SettleResponse, Erc20GasSponsoringError> {
    // Decodificar firma EIP-2612 en v, r, s
    let sig_bytes = hex::decode(
        eip2612_data.signature.strip_prefix("0x").unwrap_or(&eip2612_data.signature)
    )?;
    let r = FixedBytes::<32>::from_slice(&sig_bytes[0..32]);
    let s = FixedBytes::<32>::from_slice(&sig_bytes[32..64]);
    let v = sig_bytes[64];

    // Construir calldata para settleWith2612
    let calldata = x402Permit2Proxy::settleWith2612Call {
        permit2612: x402Permit2Proxy::EIP2612Permit {
            value: U256::from_str_radix(&eip2612_data.amount, 10)?,
            deadline: U256::from_str_radix(&eip2612_data.deadline, 10)?,
            r,
            s,
            v,
        },
        amount: U256::from_str_radix(&permit2_params.amount, 10)?,
        permit: permit2_params.permit.clone(),
        owner: permit2_params.owner,
        witness: permit2_params.witness.clone().into(),
        signature: permit2_params.signature.clone(),
    }.abi_encode();

    let receipt = provider.send_transaction(MetaTransaction {
        to: permit2_params.proxy_address,
        calldata: calldata.into(),
        confirmations: 1,
        value: None,
    }).await?;

    Ok(SettleResponse {
        success: receipt.status(),
        transaction_hash: Some(TransactionHash::Evm(receipt.transaction_hash.0)),
        ..Default::default()
    })
}
```

---

## 16. Variables de Entorno Nuevas

```bash
# .env.example - Nuevas variables para gas sponsoring

# --- Extension: erc20ApprovalGasSponsoring ---

# Habilitar la extension (default: false)
ENABLE_GAS_SPONSORING=false

# Maximo gas nativo a fondear por transaccion (en wei)
# Default: 1000000000000000 (0.001 ETH / ~$0.003 en L2)
MAX_GAS_SPONSORING_WEI=1000000000000000

# Rate limit: maximo fondeos por direccion por hora
# Default: 5
GAS_SPONSORING_RATE_LIMIT=5

# Monto minimo de pago para habilitar sponsoring (en unidades del token)
# Default: 100000 (0.10 USDC con 6 decimals)
GAS_SPONSORING_MIN_PAYMENT=100000

# Direccion del contrato x402Permit2Proxy (misma en todas las chains via CREATE2)
# Default: se usa constante hardcodeada
# X402_PERMIT2_PROXY_ADDRESS=0x...
```

---

## 17. Consideraciones para Produccion (AWS ECS)

### 17.1 Terraform

Agregar variables de entorno al task definition en `terraform/environments/production/main.tf`:

```hcl
# En el bloque environment del container definition
{
  name  = "ENABLE_GAS_SPONSORING"
  value = "false"  # Habilitar solo despues de testing completo
},
{
  name  = "MAX_GAS_SPONSORING_WEI"
  value = "1000000000000000"  # 0.001 ETH
},
{
  name  = "GAS_SPONSORING_RATE_LIMIT"
  value = "5"
},
```

### 17.2 Wallet del facilitador

El facilitador ya tiene wallets con gas nativo para settlement. El gas sponsoring usara la misma wallet pero fondeara gas al usuario. Esto implica:
- Mayor consumo de gas nativo por settlement (2-3x)
- Monitorear balance de gas del facilitador mas frecuentemente
- Considerar alertas cuando el balance cae por debajo de un umbral

### 17.3 Metricas de observabilidad

```rust
// Nuevas metricas para OpenTelemetry
tracing::info!(
    monotonic_counter.gas_sponsoring_total = 1u64,
    network = %chain.network,
    gas_funded_wei = %funding_amount,
    "Gas sponsoring ejecutado"
);

tracing::info!(
    monotonic_counter.gas_sponsoring_failures = 1u64,
    network = %chain.network,
    error = %err,
    "Gas sponsoring fallido"
);

tracing::info!(
    histogram.gas_sponsoring_amount_wei = funding_amount.to::<u64>(),
    network = %chain.network,
    "Gas sponsoring monto fondeado"
);
```

---

## 18. Resumen de Decision

### Implementar?

**SI, pero en fases.**

### Prioridad

| Fase | Extension | Tokens desbloqueados | Riesgo | Esfuerzo |
|------|-----------|---------------------|--------|----------|
| 1 | `eip2612GasSponsoring` | DAI, GHO, UNI, AAVE | Bajo | 3-4 dias |
| 2 | `erc20ApprovalGasSponsoring` | TODOS los ERC-20 | Alto | 10-15 dias |

### Prerequisitos absolutos

1. Contrato `x402Permit2Proxy` deployado y auditado
2. Verificacion de Permit2 en todas las chains objetivo
3. SDK de cliente actualizado (no depende de nosotros si usamos el SDK de upstream)
4. Tests de integracion completos en testnet

### Blockers actuales

- **x402Permit2Proxy no existe on-chain aun** - La spec de upstream lo define pero no indica si ya fue deployado. Necesitamos confirmacion del equipo upstream o deployarlo nosotros mismos.
- **SDK de cliente** - Los clientes necesitan construir payloads con la extension. Esto requiere cambios en `crates/x402-reqwest` o en el SDK que usen los clientes.
- **Auditor de contrato** - El x402Permit2Proxy maneja fondos; debe ser auditado antes de mainnet.
