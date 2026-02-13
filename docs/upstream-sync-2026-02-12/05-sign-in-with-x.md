# 05 - Extension: Sign-In-With-X (CAIP-122)

> Documento de implementacion detallado para la extension de autenticacion basada en wallets.
> Fecha: 2026-02-12 | Autor: Claude Opus 4.6 | Spec upstream: `docs/specs/extensions/sign-in-with-x.md`

---

## Tabla de Contenidos

1. [Resumen de la Feature](#1-resumen-de-la-feature)
2. [Flujo del Protocolo](#2-flujo-del-protocolo)
3. [Donde Encaja en Nuestra Arquitectura](#3-donde-encaja-en-nuestra-arquitectura)
4. [Plan de Implementacion](#4-plan-de-implementacion)
5. [Nuevos Tipos (Structs Rust)](#5-nuevos-tipos-structs-rust)
6. [Parsing del Header HTTP](#6-parsing-del-header-http)
7. [Verificacion de Firmas](#7-verificacion-de-firmas)
8. [Gestion de Nonces](#8-gestion-de-nonces)
9. [Cache de Sesiones / Direcciones que Ya Pagaron](#9-cache-de-sesiones--direcciones-que-ya-pagaron)
10. [Dependencias](#10-dependencias)
11. [Decision Arquitectonica](#11-decision-arquitectonica)
12. [Casos de Uso Concretos](#12-casos-de-uso-concretos)
13. [Evaluacion de Riesgos y Seguridad](#13-evaluacion-de-riesgos-y-seguridad)
14. [Estimacion de Esfuerzo](#14-estimacion-de-esfuerzo)
15. [Checklist de Verificacion](#15-checklist-de-verificacion)

---

## 1. Resumen de la Feature

### Que es Sign-In-With-X (SIWX)

Sign-In-With-X es una extension del protocolo x402 que implementa autenticacion basada en wallets usando el estandar [CAIP-122](https://github.com/ChainAgnostic/CAIPs/blob/main/CAIPs/caip-122.md). Permite que un cliente demuestre control sobre una direccion de wallet firmando un mensaje de desafio (challenge), sin necesidad de realizar un pago.

### Por que importa

El problema que resuelve es fundamental: **una vez que un usuario paga por un recurso, necesita una forma de volver a accederlo sin pagar de nuevo**. Sin SIWX, cada peticion HTTP requiere un nuevo pago o el servidor no tiene forma de identificar al cliente.

Con SIWX:
1. El usuario paga una vez por un recurso protegido con x402.
2. El servidor recuerda que la direccion `0xABC...` ya pago.
3. En peticiones subsiguientes, el usuario firma un challenge con su wallet.
4. El servidor verifica la firma, reconoce la direccion, y permite el acceso sin cobrar de nuevo.

### Flujo de Usuario Resumido

```
[Primera Visita]
Cliente --GET /premium--> Servidor
Servidor --402 Payment Required + extensions.sign-in-with-x--> Cliente
Cliente paga via x402 (X-Payment header)
Servidor cobra y entrega el recurso

[Visitas Posteriores]
Cliente --GET /premium + SIGN-IN-WITH-X header--> Servidor
Servidor verifica firma, reconoce wallet, entrega recurso SIN cobrar
```

### Alcance Importante

Esta extension es **Server <-> Client**. El facilitador (nuestro servicio principal en `src/main.rs`) **NO participa** en el flujo de autenticacion. La implementacion va en el middleware `x402-axum` que usan los servidores de recursos (los "sellers").

---

## 2. Flujo del Protocolo

### Paso 1: El Servidor Anuncia Soporte SIWX

Cuando un cliente hace `GET /recurso-protegido` sin header de pago ni de autenticacion, el servidor responde `402 Payment Required`. En la respuesta, ademas de los `accepts` normales de x402, incluye una seccion `extensions`:

```json
{
  "x402Version": "2",
  "accepts": [
    {
      "scheme": "exact",
      "network": "eip155:8453",
      "amount": "10000",
      "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
      "payTo": "0x209693Bc6afc0C5328bA36FaF03C514EF312287C",
      "maxTimeoutSeconds": 60,
      "extra": { "name": "USDC", "version": "2" }
    }
  ],
  "extensions": {
    "sign-in-with-x": {
      "info": {
        "domain": "api.example.com",
        "uri": "https://api.example.com/premium-data",
        "version": "1",
        "nonce": "a1b2c3d4e5f67890a1b2c3d4e5f67890",
        "issuedAt": "2024-01-15T10:30:00.000Z",
        "expirationTime": "2024-01-15T10:35:00.000Z",
        "statement": "Sign in to access premium data",
        "resources": ["https://api.example.com/premium-data"]
      },
      "supportedChains": [
        { "chainId": "eip155:8453", "type": "eip191" },
        { "chainId": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp", "type": "ed25519" }
      ],
      "schema": { ... }
    }
  }
}
```

### Paso 2: El Cliente Firma el Challenge

El cliente:
1. Lee `extensions.sign-in-with-x.info` para obtener los datos del challenge.
2. Busca en `supportedChains` la primera cadena compatible con su wallet.
3. Construye el mensaje de firma segun el formato de la cadena (EIP-4361/SIWE para EVM, SIWS para Solana).
4. Firma el mensaje con la clave privada de su wallet.
5. Construye un JSON con todos los campos del challenge + su `address` + `signature`.
6. Lo codifica en Base64.
7. Lo envia como header HTTP `SIGN-IN-WITH-X`.

### Paso 3: El Servidor Verifica y Decide

El servidor:
1. Decodifica el header `SIGN-IN-WITH-X` (Base64 -> JSON).
2. Valida los campos del mensaje:
   - `domain` debe coincidir con el host del request.
   - `issuedAt` debe ser reciente (< 5 minutos) y no estar en el futuro.
   - `expirationTime`, si presente, debe estar en el futuro.
   - `notBefore`, si presente, debe estar en el pasado.
   - `nonce` debe ser unico y no reutilizado.
3. Verifica la firma criptografica:
   - Para `eip155:*`: recupera la direccion del firmante via ECDSA recovery (EIP-191) o verificacion on-chain (EIP-1271/EIP-6492).
   - Para `solana:*`: verifica la firma Ed25519 contra la clave publica.
4. Si la firma es valida, consulta su base de datos/cache de "direcciones que ya pagaron".
5. Si la direccion ya pago, permite el acceso sin cobrar. Si no, retorna 402 normalmente.

### Formato del Mensaje EVM (EIP-4361/SIWE)

```
api.example.com wants you to sign in with your Ethereum account:
0x857b06519E91e3A54538791bDbb0E22373e36b66

Sign in to access premium data

URI: https://api.example.com/premium-data
Version: 1
Chain ID: 8453
Nonce: a1b2c3d4e5f67890a1b2c3d4e5f67890
Issued At: 2024-01-15T10:30:00.000Z
Expiration Time: 2024-01-15T10:35:00.000Z
Resources:
- https://api.example.com/premium-data
```

### Formato del Mensaje Solana (SIWS)

```
api.example.com wants you to sign in with your Solana account:
BSmWDgE9ex6dZYbiTsJGcwMEgFp8q4aWh92hdErQPeVW

Sign in to access premium data

URI: https://api.example.com/premium-data
Version: 1
Chain ID: 5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp
Nonce: a1b2c3d4e5f67890a1b2c3d4e5f67890
Issued At: 2024-01-15T10:30:00.000Z
Expiration Time: 2024-01-15T10:35:00.000Z
Resources:
- https://api.example.com/premium-data
```

---

## 3. Donde Encaja en Nuestra Arquitectura

### Componentes Afectados

| Componente | Afectado? | Razon |
|---|---|---|
| **Facilitador** (`src/main.rs`, `src/handlers.rs`) | NO | SIWX es Server<->Client. El facilitador no participa. |
| **x402-axum** (`crates/x402-axum/`) | SI - PRINCIPAL | El middleware del servidor de recursos es donde va la logica SIWX. |
| **x402-reqwest** (`crates/x402-reqwest/`) | SI - SECUNDARIO | El middleware del cliente necesita construir y enviar el header SIWX. |
| **x402-rs (types)** (`src/types_v2.rs`) | SI - TIPOS | Los tipos compartidos (challenge, proof) van aqui. |
| **x402-rs (caip2)** (`src/caip2.rs`) | NO - YA EXISTE | Ya tenemos soporte CAIP-2 completo que SIWX usara directamente. |

### Diagrama de Flujo Completo

```
                                    [Facilitador]
                                    (NO participa en SIWX)
                                         |
                                    /verify, /settle
                                         |
[Cliente]  <-- 402 + SIWX info -->  [Servidor de Recursos]
    |                                    |
    |--- SIGN-IN-WITH-X header -------->|
    |                                    |--- verifica firma
    |                                    |--- consulta cache "ya pago?"
    |<--------- 200 OK ----------------|
    |                                    |
    |--- X-Payment header ------------->|   (si no ha pagado)
    |                                    |--- /verify -> facilitador
    |                                    |--- /settle -> facilitador
    |<--------- 200 OK + receipt -------|
```

### Donde van los archivos nuevos

```
crates/x402-axum/src/
  lib.rs              -- agregar `pub mod siwx;`
  siwx/
    mod.rs            -- modulo principal, re-exports
    types.rs          -- SiwxChallenge, SiwxProof, SiwxConfig, SupportedChain
    message.rs        -- Construccion de mensajes SIWE/SIWS
    verify.rs         -- Verificacion de firmas (EIP-191, Ed25519)
    nonce.rs          -- NonceStore trait + InMemoryNonceStore
    session.rs        -- PaidAddressCache trait + InMemoryPaidAddressCache
    middleware.rs     -- SiwxLayer y SiwxMiddlewareService (Tower Layer)
    error.rs          -- SiwxError enum

crates/x402-reqwest/src/
  siwx.rs             -- Logica del cliente para firmar challenges
```

---

## 4. Plan de Implementacion

### Fase 1: Tipos Base (src/types_v2.rs + x402-axum/siwx/types.rs)

Definir todos los structs necesarios para representar el protocolo SIWX.

### Fase 2: Construccion de Mensajes (message.rs)

Implementar la construccion de mensajes SIWE (EIP-4361) y SIWS para que el servidor pueda reconstruir el mensaje que el cliente firmo, y para que el cliente pueda construir el mensaje a firmar.

### Fase 3: Verificacion de Firmas (verify.rs)

Implementar la verificacion criptografica:
- EIP-191: ECDSA recovery para EOAs.
- EIP-1271: Llamada on-chain para smart contract wallets (futuro).
- Ed25519: Verificacion directa para Solana.

### Fase 4: Gestion de Nonces (nonce.rs)

Implementar un trait `NonceStore` con una implementacion en memoria y preparar la interfaz para backends persistentes (DynamoDB, Redis).

### Fase 5: Cache de Sesiones (session.rs)

Implementar un trait `PaidAddressCache` para rastrear direcciones que ya pagaron, con implementacion en memoria y TTL configurable.

### Fase 6: Middleware Tower (middleware.rs)

Integrar todo en un `Tower::Layer` que se componga con el `X402Middleware` existente.

### Fase 7: Cliente (x402-reqwest/siwx.rs)

Implementar la logica del lado del cliente para detectar el challenge SIWX en una respuesta 402 y firmar automaticamente.

---

## 5. Nuevos Tipos (Structs Rust)

### 5.1 Tipos del Challenge (servidor -> cliente)

```rust
// crates/x402-axum/src/siwx/types.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Informacion del challenge SIWX que el servidor incluye en la respuesta 402.
///
/// Corresponde al campo `extensions.sign-in-with-x.info` en la respuesta
/// `402 Payment Required`.
///
/// Todos los campos obligatorios segun CAIP-122 estan marcados como no-Optional.
/// Los campos opcionales usan `Option<T>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiwxChallengeInfo {
    /// Dominio del servidor (ej: "api.example.com").
    /// DEBE coincidir con el host del request HTTP.
    pub domain: String,

    /// URI completa del recurso protegido.
    pub uri: String,

    /// Version del protocolo CAIP-122. Siempre "1".
    pub version: String,

    /// Nonce criptografico (32 caracteres hexadecimales).
    /// Generado por el servidor. Unico por challenge.
    pub nonce: String,

    /// Timestamp ISO 8601 de cuando se creo el challenge.
    pub issued_at: String,

    /// Declaracion legible para el usuario sobre el proposito de la firma.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,

    /// Timestamp ISO 8601 de cuando expira el challenge.
    /// Default: 5 minutos despues de `issued_at`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration_time: Option<String>,

    /// Timestamp ISO 8601 antes del cual la firma no es valida.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<String>,

    /// ID de correlacion para el request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    /// URIs asociados con el request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<String>,
}

/// Cadena soportada para autenticacion SIWX.
///
/// Cada entrada indica un chain ID en formato CAIP-2 y el tipo de firma
/// que se debe usar para esa cadena.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiwxSupportedChain {
    /// Identificador de cadena en formato CAIP-2 (ej: "eip155:8453", "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp").
    pub chain_id: String,

    /// Tipo de firma: "eip191" para EVM, "ed25519" para Solana.
    #[serde(rename = "type")]
    pub signature_type: SiwxSignatureType,

    /// Hint para el UX del cliente al firmar.
    /// Valores posibles: "eip191", "eip1271", "eip6492", "siws".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_scheme: Option<String>,
}

/// Tipos de firma soportados por SIWX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SiwxSignatureType {
    /// ECDSA recovery (Ethereum/EVM EOA wallets).
    Eip191,
    /// Ed25519 (Solana wallets).
    Ed25519,
}

/// Extension SIWX completa que va dentro de `extensions.sign-in-with-x`
/// en la respuesta 402.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiwxExtension {
    /// Metadata del challenge.
    pub info: SiwxChallengeInfo,

    /// Cadenas soportadas para autenticacion.
    pub supported_chains: Vec<SiwxSupportedChain>,

    /// JSON Schema del proof esperado (opcional, para documentacion).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
}
```

### 5.2 Tipos del Proof (cliente -> servidor)

```rust
// crates/x402-axum/src/siwx/types.rs (continuacion)

/// Proof SIWX enviado por el cliente en el header `SIGN-IN-WITH-X`.
///
/// El cliente eco-ea todos los campos del challenge y agrega su `address` y `signature`.
/// Este struct se serializa a JSON y se codifica en Base64 para el header HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiwxProof {
    /// Dominio del servidor (debe coincidir con el challenge).
    pub domain: String,

    /// Direccion de la wallet que firmo.
    /// Checksummed para EVM (ej: "0x857b06519E91e3A54538791bDbb0E22373e36b66").
    /// Base58 para Solana (ej: "BSmWDgE9ex6dZYbiTsJGcwMEgFp8q4aWh92hdErQPeVW").
    pub address: String,

    /// URI del recurso protegido.
    pub uri: String,

    /// Version del protocolo CAIP-122 (siempre "1").
    pub version: String,

    /// Chain ID en formato CAIP-2.
    pub chain_id: String,

    /// Tipo de firma utilizada.
    #[serde(rename = "type")]
    pub signature_type: SiwxSignatureType,

    /// Nonce del challenge (debe coincidir).
    pub nonce: String,

    /// Timestamp de emision del challenge.
    pub issued_at: String,

    /// Timestamp de expiracion (opcional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration_time: Option<String>,

    /// Timestamp "no antes de" (opcional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<String>,

    /// Declaracion firmada (opcional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,

    /// ID de correlacion del request (opcional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    /// URIs de recursos asociados (opcional).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<String>,

    /// Firma criptografica.
    /// Hex-encoded con prefijo "0x" para EVM.
    /// Base58-encoded para Solana.
    pub signature: String,
}
```

### 5.3 Configuracion del Middleware SIWX

```rust
// crates/x402-axum/src/siwx/types.rs (continuacion)

use std::time::Duration;

/// Configuracion para el middleware SIWX.
///
/// Define los parametros del servidor para generar challenges y verificar proofs.
#[derive(Debug, Clone)]
pub struct SiwxConfig {
    /// Dominio del servidor (ej: "api.example.com").
    /// Se usa para validar que el proof corresponde a este servidor.
    pub domain: String,

    /// Cadenas soportadas para autenticacion.
    pub supported_chains: Vec<SiwxSupportedChain>,

    /// Declaracion legible que se incluye en el challenge.
    pub statement: Option<String>,

    /// Duracion maxima de validez del challenge.
    /// Default: 5 minutos.
    pub challenge_ttl: Duration,

    /// Duracion maxima de la sesion (cuanto tiempo "recordar" que una direccion ya pago).
    /// Default: 24 horas.
    pub session_ttl: Duration,

    /// Si es true, requiere que expirationTime este presente en el proof.
    /// Default: false.
    pub require_expiration: bool,
}

impl Default for SiwxConfig {
    fn default() -> Self {
        Self {
            domain: "localhost".to_string(),
            supported_chains: vec![
                SiwxSupportedChain {
                    chain_id: "eip155:8453".to_string(),
                    signature_type: SiwxSignatureType::Eip191,
                    signature_scheme: None,
                },
            ],
            statement: None,
            challenge_ttl: Duration::from_secs(300), // 5 minutos
            session_ttl: Duration::from_secs(86400), // 24 horas
            require_expiration: false,
        }
    }
}

impl SiwxConfig {
    /// Crea una nueva configuracion con el dominio especificado.
    pub fn new(domain: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            ..Default::default()
        }
    }

    /// Agrega una cadena EVM soportada.
    pub fn with_evm_chain(mut self, chain_id: u64) -> Self {
        self.supported_chains.push(SiwxSupportedChain {
            chain_id: format!("eip155:{}", chain_id),
            signature_type: SiwxSignatureType::Eip191,
            signature_scheme: None,
        });
        self
    }

    /// Agrega Solana mainnet como cadena soportada.
    pub fn with_solana_mainnet(mut self) -> Self {
        self.supported_chains.push(SiwxSupportedChain {
            chain_id: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            signature_type: SiwxSignatureType::Ed25519,
            signature_scheme: Some("siws".to_string()),
        });
        self
    }

    /// Agrega Solana devnet como cadena soportada.
    pub fn with_solana_devnet(mut self) -> Self {
        self.supported_chains.push(SiwxSupportedChain {
            chain_id: "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1".to_string(),
            signature_type: SiwxSignatureType::Ed25519,
            signature_scheme: Some("siws".to_string()),
        });
        self
    }

    /// Configura la duracion del TTL del challenge.
    pub fn with_challenge_ttl(mut self, ttl: Duration) -> Self {
        self.challenge_ttl = ttl;
        self
    }

    /// Configura la duracion del TTL de la sesion.
    pub fn with_session_ttl(mut self, ttl: Duration) -> Self {
        self.session_ttl = ttl;
        self
    }

    /// Configura la declaracion del challenge.
    pub fn with_statement(mut self, statement: impl Into<String>) -> Self {
        self.statement = Some(statement.into());
        self
    }
}
```

---

## 6. Parsing del Header HTTP

### 6.1 Nombre del Header

El header se llama `SIGN-IN-WITH-X` (todo mayusculas con guiones). El valor es JSON codificado en Base64.

### 6.2 Implementacion del Parser

```rust
// crates/x402-axum/src/siwx/middleware.rs

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use http::HeaderMap;

use super::types::SiwxProof;
use super::error::SiwxError;

/// Nombre del header HTTP para SIWX.
pub const SIWX_HEADER_NAME: &str = "SIGN-IN-WITH-X";

/// Extrae y decodifica el proof SIWX del header HTTP.
///
/// Proceso:
/// 1. Busca el header `SIGN-IN-WITH-X` en el mapa de headers.
/// 2. Decodifica el valor de Base64.
/// 3. Parsea el JSON resultante como `SiwxProof`.
///
/// Retorna `None` si el header no esta presente.
/// Retorna `Err(SiwxError)` si el header esta presente pero es invalido.
pub fn extract_siwx_proof(headers: &HeaderMap) -> Result<Option<SiwxProof>, SiwxError> {
    let header_value = match headers.get(SIWX_HEADER_NAME) {
        Some(value) => value,
        None => return Ok(None),
    };

    let header_bytes = header_value
        .as_bytes();

    let decoded_bytes = BASE64
        .decode(header_bytes)
        .map_err(|e| SiwxError::InvalidBase64 {
            source: e.to_string(),
        })?;

    let proof: SiwxProof = serde_json::from_slice(&decoded_bytes)
        .map_err(|e| SiwxError::InvalidJson {
            source: e.to_string(),
        })?;

    Ok(Some(proof))
}
```

### 6.3 Codificacion del Header (lado cliente)

```rust
// crates/x402-reqwest/src/siwx.rs

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

/// Codifica un proof SIWX como valor de header HTTP.
///
/// Serializa el proof a JSON y lo codifica en Base64.
pub fn encode_siwx_header(proof: &SiwxProof) -> Result<String, SiwxClientError> {
    let json = serde_json::to_string(proof)
        .map_err(|e| SiwxClientError::SerializationFailed(e.to_string()))?;
    Ok(BASE64.encode(json.as_bytes()))
}
```

---

## 7. Verificacion de Firmas

### 7.1 Trait de Verificacion

```rust
// crates/x402-axum/src/siwx/verify.rs

use super::types::{SiwxProof, SiwxSignatureType};
use super::error::SiwxError;

/// Resultado de una verificacion SIWX exitosa.
#[derive(Debug, Clone)]
pub struct SiwxVerifiedIdentity {
    /// Direccion verificada de la wallet.
    pub address: String,
    /// Chain ID en formato CAIP-2.
    pub chain_id: String,
    /// Tipo de firma utilizada.
    pub signature_type: SiwxSignatureType,
}

/// Verifica un proof SIWX completo.
///
/// Este es el punto de entrada principal para la verificacion.
/// Delega a la funcion apropiada segun el tipo de firma.
///
/// # Pasos
/// 1. Determina el tipo de firma (eip191 o ed25519) del proof.
/// 2. Reconstruye el mensaje que el cliente debio firmar.
/// 3. Verifica la firma contra el mensaje reconstruido.
/// 4. Retorna la identidad verificada si todo es correcto.
pub fn verify_siwx_signature(proof: &SiwxProof) -> Result<SiwxVerifiedIdentity, SiwxError> {
    match proof.signature_type {
        SiwxSignatureType::Eip191 => verify_eip191(proof),
        SiwxSignatureType::Ed25519 => verify_ed25519(proof),
    }
}
```

### 7.2 Verificacion EIP-191 (EVM)

```rust
// crates/x402-axum/src/siwx/verify.rs (continuacion)

use alloy::primitives::{Address, Signature, keccak256};
use alloy::signers::utils::public_key_to_address;

/// Verifica una firma EIP-191 (SIWE / EIP-4361).
///
/// Proceso:
/// 1. Reconstruye el mensaje SIWE a partir de los campos del proof.
/// 2. Calcula el hash EIP-191: keccak256("\x19Ethereum Signed Message:\n" + len + message).
/// 3. Recupera la clave publica del firmante usando ECDSA recovery.
/// 4. Deriva la direccion Ethereum de la clave publica recuperada.
/// 5. Compara la direccion derivada con `proof.address` (case-insensitive).
///
/// # Dependencias
/// Usa `alloy` que ya esta en nuestro Cargo.toml raiz.
fn verify_eip191(proof: &SiwxProof) -> Result<SiwxVerifiedIdentity, SiwxError> {
    // 1. Reconstruir el mensaje SIWE
    let message = build_siwe_message(proof);
    let message_bytes = message.as_bytes();

    // 2. Hash EIP-191: "\x19Ethereum Signed Message:\n{len}{message}"
    let prefixed_message = format!(
        "\x19Ethereum Signed Message:\n{}{}",
        message_bytes.len(),
        message
    );
    let message_hash = keccak256(prefixed_message.as_bytes());

    // 3. Parsear la firma hex (0x + 65 bytes = 130 hex chars + "0x")
    let sig_hex = proof.signature.strip_prefix("0x")
        .unwrap_or(&proof.signature);
    let sig_bytes = hex::decode(sig_hex)
        .map_err(|e| SiwxError::InvalidSignatureFormat {
            reason: format!("No se pudo decodificar hex: {}", e),
        })?;

    if sig_bytes.len() != 65 {
        return Err(SiwxError::InvalidSignatureFormat {
            reason: format!("Longitud de firma incorrecta: {} (esperado: 65)", sig_bytes.len()),
        });
    }

    // 4. Recuperar la clave publica
    // alloy::primitives::Signature maneja r, s, v internamente
    let signature = Signature::try_from(sig_bytes.as_slice())
        .map_err(|e| SiwxError::InvalidSignatureFormat {
            reason: format!("Firma ECDSA invalida: {}", e),
        })?;

    let recovered_pubkey = signature
        .recover_from_prehash(&message_hash.0)
        .map_err(|e| SiwxError::SignatureVerificationFailed {
            reason: format!("ECDSA recovery fallo: {}", e),
        })?;

    // 5. Derivar direccion y comparar
    let recovered_address = public_key_to_address(&recovered_pubkey);
    let expected_address: Address = proof.address.parse()
        .map_err(|e| SiwxError::InvalidAddress {
            reason: format!("Direccion EVM invalida '{}': {}", proof.address, e),
        })?;

    if recovered_address != expected_address {
        return Err(SiwxError::AddressMismatch {
            expected: proof.address.clone(),
            recovered: format!("{:#x}", recovered_address),
        });
    }

    Ok(SiwxVerifiedIdentity {
        address: proof.address.clone(),
        chain_id: proof.chain_id.clone(),
        signature_type: SiwxSignatureType::Eip191,
    })
}
```

### 7.3 Verificacion Ed25519 (Solana)

```rust
// crates/x402-axum/src/siwx/verify.rs (continuacion)

use ed25519_dalek::{Verifier, VerifyingKey, Signature as Ed25519Signature};

/// Verifica una firma Ed25519 (SIWS / Solana).
///
/// Proceso:
/// 1. Reconstruye el mensaje SIWS a partir de los campos del proof.
/// 2. Decodifica la clave publica (Base58) del campo `address`.
/// 3. Decodifica la firma (Base58) del campo `signature`.
/// 4. Verifica la firma Ed25519 contra el mensaje.
///
/// # Dependencias
/// Usa `ed25519-dalek` (ya en Cargo.toml raiz) y `bs58` (ya en Cargo.toml raiz).
fn verify_ed25519(proof: &SiwxProof) -> Result<SiwxVerifiedIdentity, SiwxError> {
    // 1. Reconstruir el mensaje SIWS
    let message = build_siws_message(proof);
    let message_bytes = message.as_bytes();

    // 2. Decodificar clave publica desde Base58
    let pubkey_bytes = bs58::decode(&proof.address)
        .into_vec()
        .map_err(|e| SiwxError::InvalidAddress {
            reason: format!("Direccion Solana invalida (Base58): {}", e),
        })?;

    if pubkey_bytes.len() != 32 {
        return Err(SiwxError::InvalidAddress {
            reason: format!(
                "Clave publica Solana tiene longitud incorrecta: {} (esperado: 32)",
                pubkey_bytes.len()
            ),
        });
    }

    let verifying_key = VerifyingKey::from_bytes(
        pubkey_bytes.as_slice().try_into().unwrap()
    ).map_err(|e| SiwxError::InvalidAddress {
        reason: format!("Clave publica Ed25519 invalida: {}", e),
    })?;

    // 3. Decodificar firma desde Base58
    let sig_bytes = bs58::decode(&proof.signature)
        .into_vec()
        .map_err(|e| SiwxError::InvalidSignatureFormat {
            reason: format!("Firma Solana invalida (Base58): {}", e),
        })?;

    if sig_bytes.len() != 64 {
        return Err(SiwxError::InvalidSignatureFormat {
            reason: format!(
                "Firma Ed25519 tiene longitud incorrecta: {} (esperado: 64)",
                sig_bytes.len()
            ),
        });
    }

    let signature = Ed25519Signature::from_bytes(
        sig_bytes.as_slice().try_into().unwrap()
    );

    // 4. Verificar firma
    verifying_key.verify(message_bytes, &signature)
        .map_err(|e| SiwxError::SignatureVerificationFailed {
            reason: format!("Verificacion Ed25519 fallo: {}", e),
        })?;

    Ok(SiwxVerifiedIdentity {
        address: proof.address.clone(),
        chain_id: proof.chain_id.clone(),
        signature_type: SiwxSignatureType::Ed25519,
    })
}
```

### 7.4 Construccion de Mensajes

```rust
// crates/x402-axum/src/siwx/message.rs

use super::types::SiwxProof;

/// Construye un mensaje SIWE (EIP-4361) a partir de un proof.
///
/// El formato es estricto y cada linea debe coincidir exactamente
/// con lo que el cliente firmo. Cualquier discrepancia causara
/// que la verificacion de firma falle.
///
/// Formato:
/// ```text
/// {domain} wants you to sign in with your Ethereum account:
/// {address}
///
/// {statement}     <-- solo si statement esta presente
///
/// URI: {uri}
/// Version: {version}
/// Chain ID: {chain_id_numeric}  <-- solo la parte numerica de "eip155:XXXX"
/// Nonce: {nonce}
/// Issued At: {issued_at}
/// Expiration Time: {expiration_time}  <-- solo si presente
/// Not Before: {not_before}            <-- solo si presente
/// Request ID: {request_id}            <-- solo si presente
/// Resources:                          <-- solo si resources no esta vacio
/// - {resource_1}
/// - {resource_2}
/// ```
pub fn build_siwe_message(proof: &SiwxProof) -> String {
    let mut msg = String::new();

    // Linea 1: dominio + cuenta
    msg.push_str(&format!(
        "{} wants you to sign in with your Ethereum account:\n",
        proof.domain
    ));
    msg.push_str(&proof.address);
    msg.push('\n');

    // Linea vacia + statement (opcional)
    msg.push('\n');
    if let Some(ref statement) = proof.statement {
        msg.push_str(statement);
        msg.push('\n');
    }

    // Linea vacia antes de campos URI
    msg.push('\n');

    // Campos obligatorios
    msg.push_str(&format!("URI: {}\n", proof.uri));
    msg.push_str(&format!("Version: {}\n", proof.version));

    // Chain ID: extraer la parte numerica de "eip155:XXXX"
    let chain_id_numeric = proof.chain_id
        .split(':')
        .nth(1)
        .unwrap_or(&proof.chain_id);
    msg.push_str(&format!("Chain ID: {}\n", chain_id_numeric));

    msg.push_str(&format!("Nonce: {}\n", proof.nonce));
    msg.push_str(&format!("Issued At: {}", proof.issued_at));

    // Campos opcionales
    if let Some(ref exp) = proof.expiration_time {
        msg.push_str(&format!("\nExpiration Time: {}", exp));
    }

    if let Some(ref nb) = proof.not_before {
        msg.push_str(&format!("\nNot Before: {}", nb));
    }

    if let Some(ref rid) = proof.request_id {
        msg.push_str(&format!("\nRequest ID: {}", rid));
    }

    if !proof.resources.is_empty() {
        msg.push_str("\nResources:");
        for resource in &proof.resources {
            msg.push_str(&format!("\n- {}", resource));
        }
    }

    msg
}

/// Construye un mensaje SIWS (Sign-In With Solana) a partir de un proof.
///
/// El formato es identico al SIWE pero usa "Solana account" en lugar
/// de "Ethereum account", y el Chain ID es el genesis hash (no numerico).
pub fn build_siws_message(proof: &SiwxProof) -> String {
    let mut msg = String::new();

    // Linea 1: dominio + cuenta
    msg.push_str(&format!(
        "{} wants you to sign in with your Solana account:\n",
        proof.domain
    ));
    msg.push_str(&proof.address);
    msg.push('\n');

    // Linea vacia + statement (opcional)
    msg.push('\n');
    if let Some(ref statement) = proof.statement {
        msg.push_str(statement);
        msg.push('\n');
    }

    // Linea vacia antes de campos URI
    msg.push('\n');

    // Campos obligatorios
    msg.push_str(&format!("URI: {}\n", proof.uri));
    msg.push_str(&format!("Version: {}\n", proof.version));

    // Chain ID: para Solana es el genesis hash (parte despues de "solana:")
    let chain_id_reference = proof.chain_id
        .split(':')
        .nth(1)
        .unwrap_or(&proof.chain_id);
    msg.push_str(&format!("Chain ID: {}\n", chain_id_reference));

    msg.push_str(&format!("Nonce: {}\n", proof.nonce));
    msg.push_str(&format!("Issued At: {}", proof.issued_at));

    // Campos opcionales (identico a SIWE)
    if let Some(ref exp) = proof.expiration_time {
        msg.push_str(&format!("\nExpiration Time: {}", exp));
    }

    if let Some(ref nb) = proof.not_before {
        msg.push_str(&format!("\nNot Before: {}", nb));
    }

    if let Some(ref rid) = proof.request_id {
        msg.push_str(&format!("\nRequest ID: {}", rid));
    }

    if !proof.resources.is_empty() {
        msg.push_str("\nResources:");
        for resource in &proof.resources {
            msg.push_str(&format!("\n- {}", resource));
        }
    }

    msg
}
```

---

## 8. Gestion de Nonces

### 8.1 Trait NonceStore

```rust
// crates/x402-axum/src/siwx/nonce.rs

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use super::error::SiwxError;

/// Trait para almacenes de nonces SIWX.
///
/// Los nonces previenen ataques de replay. Cada challenge genera un nonce
/// unico, y el servidor debe rastrear cuales nonces ya fueron usados.
///
/// # Implementaciones
/// - `InMemoryNonceStore`: Para desarrollo y servicios single-instance.
/// - Futuro: `DynamoDbNonceStore`, `RedisNonceStore` para produccion multi-instancia.
pub trait NonceStore: Send + Sync + 'static {
    /// Genera un nuevo nonce criptograficamente seguro.
    ///
    /// Retorna un string de 32 caracteres hexadecimales.
    /// El nonce se registra como "emitido" con un TTL.
    fn generate(&self) -> Pin<Box<dyn Future<Output = Result<String, SiwxError>> + Send>>;

    /// Valida y consume un nonce.
    ///
    /// Retorna `Ok(())` si el nonce es valido (fue emitido y no ha sido usado).
    /// Retorna `Err(SiwxError::NonceInvalid)` si el nonce no existe o ya fue usado.
    ///
    /// IMPORTANTE: Esta operacion DEBE ser atomica. El nonce se consume
    /// al validarlo, impidiendo su reutilizacion.
    fn validate_and_consume(
        &self,
        nonce: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SiwxError>> + Send>>;
}
```

### 8.2 Implementacion en Memoria

```rust
// crates/x402-axum/src/siwx/nonce.rs (continuacion)

use dashmap::DashMap;
use rand::Rng;
use std::sync::Arc;

/// Almacen de nonces en memoria con TTL automatico.
///
/// Adecuado para desarrollo y servicios single-instance.
/// Para produccion multi-instancia, usar una implementacion con backend
/// compartido (DynamoDB, Redis).
///
/// # Limpieza
/// Los nonces expirados se limpian periodicamente con una tarea en background.
/// Se puede usar `start_cleanup_task()` para iniciar la limpieza automatica.
#[derive(Clone)]
pub struct InMemoryNonceStore {
    /// Mapa de nonces emitidos -> timestamp de creacion.
    nonces: Arc<DashMap<String, Instant>>,
    /// Tiempo de vida maximo de un nonce.
    ttl: Duration,
}

impl InMemoryNonceStore {
    /// Crea un nuevo store con el TTL especificado.
    pub fn new(ttl: Duration) -> Self {
        Self {
            nonces: Arc::new(DashMap::new()),
            ttl,
        }
    }

    /// Crea un nuevo store con TTL por defecto de 5 minutos.
    pub fn default_ttl() -> Self {
        Self::new(Duration::from_secs(300))
    }

    /// Inicia una tarea en background que limpia nonces expirados
    /// cada `interval`.
    ///
    /// Retorna un `JoinHandle` que se puede usar para cancelar la tarea.
    pub fn start_cleanup_task(
        &self,
        interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let nonces = Arc::clone(&self.nonces);
        let ttl = self.ttl;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                let now = Instant::now();
                nonces.retain(|_, created_at| {
                    now.duration_since(*created_at) < ttl
                });
            }
        })
    }
}

impl NonceStore for InMemoryNonceStore {
    fn generate(&self) -> Pin<Box<dyn Future<Output = Result<String, SiwxError>> + Send>> {
        let nonces = Arc::clone(&self.nonces);
        Box::pin(async move {
            let mut rng = rand::thread_rng();
            let nonce: String = (0..16)
                .map(|_| format!("{:02x}", rng.gen::<u8>()))
                .collect();
            nonces.insert(nonce.clone(), Instant::now());
            Ok(nonce)
        })
    }

    fn validate_and_consume(
        &self,
        nonce: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SiwxError>> + Send>> {
        let nonces = Arc::clone(&self.nonces);
        let nonce = nonce.to_string();
        let ttl = self.ttl;
        Box::pin(async move {
            // Atomicamente remover el nonce y verificar que existia
            match nonces.remove(&nonce) {
                Some((_, created_at)) => {
                    // Verificar que no ha expirado
                    if Instant::now().duration_since(created_at) > ttl {
                        Err(SiwxError::NonceExpired { nonce })
                    } else {
                        Ok(())
                    }
                }
                None => Err(SiwxError::NonceInvalid { nonce }),
            }
        })
    }
}
```

---

## 9. Cache de Sesiones / Direcciones que Ya Pagaron

### 9.1 Trait PaidAddressCache

```rust
// crates/x402-axum/src/siwx/session.rs

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Trait para caches de direcciones que ya pagaron.
///
/// Permite al servidor recordar que una wallet ya pago por un recurso,
/// evitando cobrar de nuevo cuando el usuario se autentica via SIWX.
///
/// # Clave de Cache
/// La clave es una tupla `(address, resource_uri)`, ya que una direccion
/// puede haber pagado por un recurso pero no por otro.
///
/// # Implementaciones
/// - `InMemoryPaidAddressCache`: Para desarrollo y servicios single-instance.
/// - Futuro: `DynamoDbPaidAddressCache`, `RedisPaidAddressCache`.
pub trait PaidAddressCache: Send + Sync + 'static {
    /// Registra que una direccion ha pagado por un recurso.
    ///
    /// La entrada tiene un TTL configurado despues del cual se considera expirada.
    fn mark_paid(
        &self,
        address: &str,
        resource_uri: &str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;

    /// Verifica si una direccion ya pago por un recurso.
    ///
    /// Retorna `true` si la direccion tiene una entrada vigente en el cache.
    fn has_paid(
        &self,
        address: &str,
        resource_uri: &str,
    ) -> Pin<Box<dyn Future<Output = bool> + Send>>;
}
```

### 9.2 Implementacion en Memoria

```rust
// crates/x402-axum/src/siwx/session.rs (continuacion)

use dashmap::DashMap;
use std::sync::Arc;
use std::time::Instant;

/// Cache en memoria de direcciones que ya pagaron, con TTL.
///
/// Usa `DashMap` para acceso concurrente sin locks.
/// Cada entrada almacena el timestamp de cuando se registro el pago.
#[derive(Clone)]
pub struct InMemoryPaidAddressCache {
    /// Mapa de (address_lower + "|" + resource_uri) -> timestamp de pago.
    entries: Arc<DashMap<String, Instant>>,
    /// TTL de las entradas del cache.
    ttl: Duration,
}

impl InMemoryPaidAddressCache {
    /// Crea un nuevo cache con el TTL especificado.
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            ttl,
        }
    }

    /// Crea un nuevo cache con TTL por defecto de 24 horas.
    pub fn default_ttl() -> Self {
        Self::new(Duration::from_secs(86400))
    }

    /// Construye la clave del cache.
    /// Normaliza la direccion a lowercase para comparaciones case-insensitive.
    fn cache_key(address: &str, resource_uri: &str) -> String {
        format!("{}|{}", address.to_lowercase(), resource_uri)
    }

    /// Inicia una tarea de limpieza periodica.
    pub fn start_cleanup_task(
        &self,
        interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let entries = Arc::clone(&self.entries);
        let ttl = self.ttl;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                let now = Instant::now();
                entries.retain(|_, paid_at| {
                    now.duration_since(*paid_at) < ttl
                });
            }
        })
    }
}

impl PaidAddressCache for InMemoryPaidAddressCache {
    fn mark_paid(
        &self,
        address: &str,
        resource_uri: &str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let entries = Arc::clone(&self.entries);
        let key = Self::cache_key(address, resource_uri);
        Box::pin(async move {
            entries.insert(key, Instant::now());
        })
    }

    fn has_paid(
        &self,
        address: &str,
        resource_uri: &str,
    ) -> Pin<Box<dyn Future<Output = bool> + Send>> {
        let entries = Arc::clone(&self.entries);
        let key = Self::cache_key(address, resource_uri);
        let ttl = self.ttl;
        Box::pin(async move {
            match entries.get(&key) {
                Some(entry) => {
                    let paid_at = *entry.value();
                    Instant::now().duration_since(paid_at) < ttl
                }
                None => false,
            }
        })
    }
}
```

---

## 10. Dependencias

### 10.1 Dependencias que YA Existen en el Proyecto

Estas dependencias ya estan en el workspace y no requieren ningun cambio en `Cargo.toml`:

| Dependencia | Version | Uso en SIWX | Ubicacion Actual |
|---|---|---|---|
| `alloy` | 1.0.12 | Verificacion EIP-191 (ECDSA recovery, keccak256, Address) | Cargo.toml raiz + x402-reqwest |
| `ed25519-dalek` | 2.1 | Verificacion Ed25519 (Solana) | Cargo.toml raiz |
| `bs58` | 0.5 | Decodificacion Base58 (direcciones/firmas Solana) | Cargo.toml raiz |
| `hex` | 0.4 | Decodificacion hex (firmas EVM) | Cargo.toml raiz |
| `base64` | 0.22.1 | Codificacion/decodificacion del header SIWX | Cargo.toml raiz |
| `serde` + `serde_json` | 1.0.x | Serializacion de tipos SIWX | Todo el workspace |
| `dashmap` | 6.1.0 | Almacen concurrente para nonces y sesiones | Cargo.toml raiz |
| `rand` | 0.8 | Generacion de nonces criptograficos | Cargo.toml raiz |
| `tokio` | 1.49.0 | Tareas async para limpieza de caches | Todo el workspace |
| `thiserror` | 2.0.18 | Tipos de error derivados | Todo el workspace |
| `http` | 1.4.0 | HeaderMap, StatusCode | x402-axum |
| `tower` | 0.5.3 | Layer, Service para el middleware | x402-axum |
| `once_cell` | 1.21.3 | Lazy statics | x402-axum |
| `tracing` | 0.1.44 | Logging/telemetria (opcional) | Todo el workspace |

### 10.2 Dependencias NUEVAS Necesarias

| Dependencia | Version Sugerida | Uso | Donde Agregarla |
|---|---|---|---|
| `chrono` | 0.4 | Parsing y validacion de timestamps ISO 8601 | `crates/x402-axum/Cargo.toml` |

**Nota sobre `chrono`**: Actualmente el proyecto no tiene una dependencia de parsing de timestamps ISO 8601. Podriamos implementarlo manualmente o usar `chrono`. Dado que SIWX necesita validar `issuedAt`, `expirationTime`, y `notBefore` con precision de timestamps, `chrono` es la opcion mas segura.

**Alternativa sin `chrono`**: Si se quiere evitar una dependencia nueva, se puede usar `time` (que ya es dependencia transitiva de varios crates del workspace) o parsear timestamps ISO 8601 manualmente con regex (ya tenemos `regex` 1.11.1). Sin embargo, `chrono` es el estandar de facto en el ecosistema Rust para timestamps.

### 10.3 Dependencias OPCIONALES (Futuras)

| Dependencia | Uso | Cuando Agregarla |
|---|---|---|
| `siwe` (crate) | Parser oficial de mensajes SIWE | Si queremos usar la implementacion oficial en lugar de nuestro parser manual. La crate `siwe` (v0.6+) implementa el spec EIP-4361 completo incluyendo verificacion. Considerarlo si nuestro parser manual tiene bugs. |
| `redis` | Backend persistente para NonceStore/PaidAddressCache | Cuando se necesite multi-instancia en produccion. |
| `aws-sdk-dynamodb` | Backend DynamoDB para NonceStore/PaidAddressCache | Ya esta en el Cargo.toml raiz. Reutilizable si queremos persistencia en AWS. |

### 10.4 Cambios al Cargo.toml de x402-axum

```toml
# crates/x402-axum/Cargo.toml - cambios necesarios

[dependencies]
# ... dependencias existentes ...

# SIWX - Nuevas dependencias
chrono = { version = "0.4", features = ["serde"] }
# Las siguientes ya son dependencias transitivas pero las
# necesitamos como dependencias directas para SIWX:
hex = { version = "0.4" }
base64 = { version = "0.22.1" }
rand = { version = "0.8" }
dashmap = { version = "6.1.0" }
tokio = { version = "1.49.0", features = ["time"] }

# SIWX - Dependencias del workspace que necesitamos referenciar
alloy = { version = "1.0.12" }
ed25519-dalek = { version = "2.1", features = ["rand_core"] }
bs58 = { version = "0.5" }

[features]
default = []
telemetry = ["dep:tracing", "x402-rs/telemetry"]
# Nuevo feature flag para SIWX
siwx = []
# SIWX con soporte EVM (incluye verificacion EIP-191)
siwx-evm = ["siwx"]
# SIWX con soporte Solana (incluye verificacion Ed25519)
siwx-solana = ["siwx"]
# SIWX completo (todas las cadenas)
siwx-full = ["siwx-evm", "siwx-solana"]
```

**Nota sobre feature flags**: Usar feature flags permite que usuarios de `x402-axum` que no necesiten SIWX no paguen el costo de compilacion de las dependencias criptograficas adicionales. El crate ya usa este patron con `telemetry`.

---

## 11. Decision Arquitectonica

### Pregunta Central: Modulo dentro de x402-axum vs. Crate Independiente?

#### Opcion A: Modulo dentro de `crates/x402-axum/src/siwx/` (RECOMENDADA)

**Ventajas:**
- SIWX esta intimamente ligado al middleware x402. Siempre se usa junto con `X402Middleware`.
- Comparte tipos con el middleware existente (`PaymentRequirements`, `PaymentRequiredResponse`).
- Los usuarios instalan un solo crate (`x402-axum`) con un feature flag (`siwx`).
- Sigue el patron del upstream que definira SIWX como parte del flujo del middleware.
- Menor friccion para el usuario: `x402_axum::siwx::SiwxConfig` en lugar de una dependencia separada.
- El middleware SIWX necesita acceso a la respuesta 402 para inyectar la extension, lo cual es natural dentro del mismo crate.

**Desventajas:**
- Aumenta el tamano del crate `x402-axum`.
- Las dependencias criptograficas (alloy, ed25519-dalek) se agregan al crate. (Mitigado con feature flags.)

#### Opcion B: Crate independiente `x402-siwx`

**Ventajas:**
- Separacion maxima de concerns.
- Se puede versionar independientemente.

**Desventajas:**
- Duplicacion de tipos (o dependencia circular con x402-axum).
- El usuario debe instalar y configurar dos crates.
- El middleware SIWX necesita integrarse con el flujo de `X402Middleware`, creando acoplamiento de todas formas.

### Decision: Opcion A - Modulo dentro de x402-axum

La implementacion ira en `crates/x402-axum/src/siwx/` como un modulo con feature flag `siwx`. Esta decision se alinea con:
1. El spec upstream que define SIWX como una extension del protocolo x402, no como un protocolo separado.
2. El patron de diseno de `x402-axum` que ya agrupa middleware de pago, facilitator client, y price tags en un solo crate.
3. La necesidad practica de que SIWX interactue directamente con la respuesta 402 generada por `X402Middleware`.

### Integracion con X402Middleware Existente

La integracion se hace extendiendo `X402Middleware` con metodos de configuracion SIWX:

```rust
// crates/x402-axum/src/layer.rs - Extensiones para SIWX

#[cfg(feature = "siwx")]
use crate::siwx::{
    SiwxConfig, NonceStore, PaidAddressCache,
    InMemoryNonceStore, InMemoryPaidAddressCache,
};

#[cfg(feature = "siwx")]
impl<F> X402Middleware<F>
where
    F: Clone,
{
    /// Habilita la extension SIWX en este middleware.
    ///
    /// Cuando esta habilitada, la respuesta 402 incluira los datos del challenge SIWX
    /// en `extensions.sign-in-with-x`. Las peticiones con el header `SIGN-IN-WITH-X`
    /// seran verificadas automaticamente.
    ///
    /// # Ejemplo
    ///
    /// ```rust,ignore
    /// use x402_axum::siwx::{SiwxConfig, InMemoryNonceStore, InMemoryPaidAddressCache};
    ///
    /// let siwx_config = SiwxConfig::new("api.example.com")
    ///     .with_evm_chain(8453)
    ///     .with_solana_mainnet()
    ///     .with_statement("Sign in to access premium content");
    ///
    /// let x402 = X402Middleware::try_from("https://facilitator.example.com/")
    ///     .unwrap()
    ///     .with_siwx(
    ///         siwx_config,
    ///         InMemoryNonceStore::default_ttl(),
    ///         InMemoryPaidAddressCache::default_ttl(),
    ///     );
    /// ```
    pub fn with_siwx<N, P>(
        &self,
        config: SiwxConfig,
        nonce_store: N,
        paid_cache: P,
    ) -> Self
    where
        N: NonceStore,
        P: PaidAddressCache,
    {
        let mut this = self.clone();
        // Almacenar la configuracion SIWX como campo del middleware
        // (requiere agregar campos a X402Middleware)
        this.siwx = Some(Arc::new(SiwxState {
            config,
            nonce_store: Box::new(nonce_store),
            paid_cache: Box::new(paid_cache),
        }));
        this
    }
}
```

### Nuevo Campo en X402Middleware

```rust
// Agregar a la struct X402Middleware en layer.rs:

#[cfg(feature = "siwx")]
/// Estado SIWX opcional. Si esta presente, el middleware soporta autenticacion SIWX.
siwx: Option<Arc<SiwxState>>,
```

Donde `SiwxState` es:

```rust
// crates/x402-axum/src/siwx/mod.rs

use std::sync::Arc;

/// Estado compartido del middleware SIWX.
///
/// Contiene la configuracion, el almacen de nonces, y el cache de sesiones.
/// Se comparte entre todas las instancias del middleware via `Arc`.
pub struct SiwxState {
    pub config: SiwxConfig,
    pub nonce_store: Box<dyn NonceStore>,
    pub paid_cache: Box<dyn PaidAddressCache>,
}
```

### Flujo Modificado en handle_request

El flujo actual de `X402MiddlewareService::handle_request` es:

```
1. Extraer X-Payment header
2. Si no existe -> 402 Payment Required
3. Si existe -> verificar pago -> settle -> 200
```

Con SIWX, el flujo se convierte en:

```
1. Verificar si hay header SIGN-IN-WITH-X
   1a. Si existe -> verificar firma -> consultar cache "ya pago?"
       - Si ya pago -> 200 (bypass pago)
       - Si no pago -> 402 con extension SIWX
2. Si no hay SIWX header -> flujo x402 normal:
   2a. Extraer X-Payment header
   2b. Si no existe -> 402 Payment Required (+ extension SIWX si habilitada)
   2c. Si existe -> verificar pago -> settle -> marcar direccion como "pagada" -> 200
```

```rust
// Pseudocodigo del flujo modificado en handle_request:

pub async fn handle_request(self, inner: S, req: Request) -> Response {
    #[cfg(feature = "siwx")]
    if let Some(ref siwx_state) = self.siwx {
        // Intentar autenticacion SIWX primero
        match extract_siwx_proof(req.headers()) {
            Ok(Some(proof)) => {
                // Hay un header SIWX: validar campos temporales y nonce
                if let Err(e) = validate_siwx_fields(&proof, &siwx_state.config) {
                    // Fields invalidos -> 402 con error
                    return siwx_error_response(e, &self.payment_requirements).into_response();
                }

                // Validar y consumir el nonce
                if let Err(e) = siwx_state.nonce_store.validate_and_consume(&proof.nonce).await {
                    return siwx_error_response(e, &self.payment_requirements).into_response();
                }

                // Verificar la firma criptografica
                match verify_siwx_signature(&proof) {
                    Ok(identity) => {
                        // Firma valida: verificar si ya pago
                        let resource_uri = /* construir URI del recurso */;
                        if siwx_state.paid_cache.has_paid(&identity.address, &resource_uri).await {
                            // Ya pago -> permitir acceso sin cobrar
                            return Self::call_inner(inner, req).await
                                .map(|r| r.into_response())
                                .unwrap_or_else(|e| e.into_response());
                        }
                        // No ha pagado -> caer al flujo normal de pago
                    }
                    Err(e) => {
                        return siwx_error_response(e, &self.payment_requirements).into_response();
                    }
                }
            }
            Ok(None) => {
                // Sin header SIWX -> flujo normal
            }
            Err(e) => {
                // Header SIWX malformado -> error
                return siwx_error_response(e, &self.payment_requirements).into_response();
            }
        }
    }

    // Flujo x402 normal (existente)
    let payment_payload = match self.extract_payment_payload(req.headers()).await {
        Ok(pp) => pp,
        Err(err) => {
            // Inyectar extension SIWX en la respuesta 402 si esta habilitada
            #[cfg(feature = "siwx")]
            if let Some(ref siwx_state) = self.siwx {
                // TODO: modificar la respuesta 402 para incluir extensions.sign-in-with-x
            }
            return err.into_response();
        }
    };

    // ... verificar y settle como antes ...

    // Despues de un settle exitoso, marcar la direccion como "pagada"
    #[cfg(feature = "siwx")]
    if let Some(ref siwx_state) = self.siwx {
        let payer_address = /* extraer del payment_payload */;
        let resource_uri = /* construir URI del recurso */;
        siwx_state.paid_cache.mark_paid(&payer_address, &resource_uri).await;
    }

    // ... retornar respuesta con X-Payment-Response header ...
}
```

---

## 12. Casos de Uso Concretos

### Caso 1: API de Datos Premium (Nuestros Usuarios Principales)

Un servidor de API meteorologica protege `/api/weather/premium` con x402.

```rust
use x402_axum::{X402Middleware, IntoPriceTag};
use x402_axum::siwx::{SiwxConfig, InMemoryNonceStore, InMemoryPaidAddressCache};
use x402_rs::network::{Network, USDCDeployment};

// Configuracion del servidor
let siwx_config = SiwxConfig::new("api.weather.com")
    .with_evm_chain(8453)     // Base
    .with_evm_chain(137)      // Polygon
    .with_solana_mainnet()
    .with_statement("Sign in to access premium weather data")
    .with_session_ttl(Duration::from_secs(3600)); // 1 hora

let usdc = USDCDeployment::by_network(Network::Base)
    .pay_to("0xSELLER_ADDRESS");

let x402 = X402Middleware::try_from("https://facilitator.ultravioletadao.xyz/")
    .unwrap()
    .with_price_tag(usdc.amount(0.01).unwrap())
    .with_siwx(
        siwx_config,
        InMemoryNonceStore::default_ttl(),
        InMemoryPaidAddressCache::new(Duration::from_secs(3600)),
    );

let app = Router::new()
    .route("/api/weather/premium", get(premium_weather).layer(x402));
```

**Flujo del usuario:**
1. `GET /api/weather/premium` -> 402 con challenge SIWX + opciones de pago
2. El cliente paga 0.01 USDC (primera vez)
3. El servidor guarda que `0xUSER...` pago
4. El usuario vuelve a pedir `GET /api/weather/premium` con header `SIGN-IN-WITH-X`
5. El servidor verifica la firma, ve que ya pago, y retorna los datos sin cobrar
6. Despues de 1 hora, la sesion expira y el usuario debe pagar de nuevo

### Caso 2: Contenido Digital con Acceso Ilimitado

Un servicio de noticias vende acceso mensual.

```rust
let siwx_config = SiwxConfig::new("news.example.com")
    .with_evm_chain(8453)
    .with_statement("Sign in to access unlimited articles")
    .with_session_ttl(Duration::from_secs(30 * 24 * 3600)); // 30 dias

// Precio mas alto, pero acceso por 30 dias
let usdc = USDCDeployment::by_network(Network::Base)
    .pay_to("0xNEWS_WALLET");

let x402 = X402Middleware::try_from("https://facilitator.ultravioletadao.xyz/")
    .unwrap()
    .with_price_tag(usdc.amount(5.00).unwrap())
    .with_siwx(
        siwx_config,
        InMemoryNonceStore::default_ttl(),
        InMemoryPaidAddressCache::new(Duration::from_secs(30 * 24 * 3600)),
    );
```

### Caso 3: API Multi-chain para Agentes AI

Un agente AI puede pagar desde cualquier cadena:

```rust
let siwx_config = SiwxConfig::new("ai-tools.example.com")
    .with_evm_chain(8453)     // Base
    .with_evm_chain(10)       // Optimism
    .with_evm_chain(42161)    // Arbitrum
    .with_evm_chain(137)      // Polygon
    .with_solana_mainnet()
    .with_statement("Sign in to use AI tools");

// Aceptar pago en multiples cadenas
let base_usdc = USDCDeployment::by_network(Network::Base)
    .pay_to("0xSELLER").amount(0.05).unwrap();
let optimism_usdc = USDCDeployment::by_network(Network::Optimism)
    .pay_to("0xSELLER").amount(0.05).unwrap();
let polygon_usdc = USDCDeployment::by_network(Network::Polygon)
    .pay_to("0xSELLER").amount(0.05).unwrap();

let x402 = X402Middleware::try_from("https://facilitator.ultravioletadao.xyz/")
    .unwrap()
    .with_price_tag(base_usdc)
    .or_price_tag(optimism_usdc)
    .or_price_tag(polygon_usdc)
    .with_siwx(
        siwx_config,
        InMemoryNonceStore::default_ttl(),
        InMemoryPaidAddressCache::default_ttl(),
    );
```

---

## 13. Evaluacion de Riesgos y Seguridad

### 13.1 Riesgos Criptograficos

| Riesgo | Severidad | Mitigacion |
|---|---|---|
| **Replay de firmas** | CRITICA | Nonces unicos + consumo atomico. Cada nonce se invalida al primer uso. |
| **Cross-domain signature reuse** | ALTA | El campo `domain` esta firmado y se valida contra el host del request. |
| **Cross-chain signature reuse** | ALTA | El campo `chainId` esta firmado. Los formatos de mensaje son distintos entre EVM y Solana. |
| **Timestamp manipulation** | MEDIA | Validar que `issuedAt` sea reciente (<5min), `expirationTime` en el futuro, `notBefore` en el pasado. |
| **Firma de smart contract wallet (EIP-1271)** | BAJA (no soportado inicialmente) | Solo soportamos EOAs (EIP-191) en la primera version. EIP-1271 requiere RPC calls y se implementara en una fase posterior. |

### 13.2 Riesgos de Implementacion

| Riesgo | Severidad | Mitigacion |
|---|---|---|
| **Formato de mensaje incorrecto** | ALTA | La reconstruccion del mensaje SIWE/SIWS DEBE ser byte-perfect. Un solo byte de diferencia hara que la verificacion falle. Tests exhaustivos con vectores de prueba conocidos. |
| **Race condition en nonces** | MEDIA | `DashMap::remove()` es atomico. Para implementaciones con backend externo (Redis/DynamoDB), usar operaciones atomicas (SETNX, ConditionExpression). |
| **Memory leak en caches** | BAJA | Tareas de limpieza periodica con `start_cleanup_task()`. Configurar intervalos de limpieza razonables (ej: cada 5 minutos para nonces, cada hora para sesiones). |
| **DoS via generacion masiva de nonces** | MEDIA | Limitar la tasa de generacion de nonces. Considerar un rate limiter por IP en el challenge endpoint. |

### 13.3 Validacion de Campos (Obligatoria)

```rust
// crates/x402-axum/src/siwx/verify.rs

use chrono::{DateTime, Utc};

/// Valida los campos temporales y de dominio de un proof SIWX.
///
/// Esta funcion DEBE llamarse ANTES de la verificacion criptografica,
/// ya que es mas barata y rechaza proofs invalidos rapidamente.
pub fn validate_siwx_fields(
    proof: &SiwxProof,
    config: &SiwxConfig,
) -> Result<(), SiwxError> {
    // 1. Dominio debe coincidir exactamente
    if proof.domain != config.domain {
        return Err(SiwxError::DomainMismatch {
            expected: config.domain.clone(),
            received: proof.domain.clone(),
        });
    }

    // 2. Version debe ser "1"
    if proof.version != "1" {
        return Err(SiwxError::UnsupportedVersion {
            version: proof.version.clone(),
        });
    }

    // 3. issuedAt debe ser parseable y reciente
    let issued_at = DateTime::parse_from_rfc3339(&proof.issued_at)
        .map_err(|e| SiwxError::InvalidTimestamp {
            field: "issuedAt".to_string(),
            value: proof.issued_at.clone(),
            reason: e.to_string(),
        })?
        .with_timezone(&Utc);

    let now = Utc::now();

    // issuedAt no debe estar en el futuro (con 30 segundos de tolerancia por clock skew)
    if issued_at > now + chrono::Duration::seconds(30) {
        return Err(SiwxError::TimestampInFuture {
            field: "issuedAt".to_string(),
            value: proof.issued_at.clone(),
        });
    }

    // issuedAt no debe ser demasiado antiguo
    let max_age = chrono::Duration::from_std(config.challenge_ttl)
        .unwrap_or(chrono::Duration::seconds(300));
    if now - issued_at > max_age {
        return Err(SiwxError::ChallengeExpired {
            issued_at: proof.issued_at.clone(),
            max_age_seconds: config.challenge_ttl.as_secs(),
        });
    }

    // 4. expirationTime (si presente) debe estar en el futuro
    if let Some(ref exp) = proof.expiration_time {
        let expiration = DateTime::parse_from_rfc3339(exp)
            .map_err(|e| SiwxError::InvalidTimestamp {
                field: "expirationTime".to_string(),
                value: exp.clone(),
                reason: e.to_string(),
            })?
            .with_timezone(&Utc);

        if expiration < now {
            return Err(SiwxError::ProofExpired {
                expiration_time: exp.clone(),
            });
        }
    }

    // 5. notBefore (si presente) debe estar en el pasado
    if let Some(ref nb) = proof.not_before {
        let not_before = DateTime::parse_from_rfc3339(nb)
            .map_err(|e| SiwxError::InvalidTimestamp {
                field: "notBefore".to_string(),
                value: nb.clone(),
                reason: e.to_string(),
            })?
            .with_timezone(&Utc);

        if not_before > now {
            return Err(SiwxError::NotYetValid {
                not_before: nb.clone(),
            });
        }
    }

    // 6. chainId debe estar en las cadenas soportadas
    let chain_supported = config.supported_chains.iter()
        .any(|sc| sc.chain_id == proof.chain_id);
    if !chain_supported {
        return Err(SiwxError::UnsupportedChain {
            chain_id: proof.chain_id.clone(),
        });
    }

    // 7. URI debe empezar con el dominio esperado
    if !proof.uri.contains(&config.domain) {
        return Err(SiwxError::UriMismatch {
            expected_domain: config.domain.clone(),
            received_uri: proof.uri.clone(),
        });
    }

    Ok(())
}
```

### 13.4 Enum de Errores

```rust
// crates/x402-axum/src/siwx/error.rs

use thiserror::Error;

/// Errores que pueden ocurrir durante el procesamiento SIWX.
#[derive(Debug, Error)]
pub enum SiwxError {
    // --- Errores de parsing ---

    #[error("Header SIGN-IN-WITH-X contiene Base64 invalido: {source}")]
    InvalidBase64 { source: String },

    #[error("Header SIGN-IN-WITH-X contiene JSON invalido: {source}")]
    InvalidJson { source: String },

    // --- Errores de validacion de campos ---

    #[error("Dominio no coincide: esperado '{expected}', recibido '{received}'")]
    DomainMismatch { expected: String, received: String },

    #[error("Version CAIP-122 no soportada: '{version}' (solo se soporta '1')")]
    UnsupportedVersion { version: String },

    #[error("Timestamp invalido en campo '{field}': '{value}' ({reason})")]
    InvalidTimestamp {
        field: String,
        value: String,
        reason: String,
    },

    #[error("El campo '{field}' tiene un timestamp en el futuro: '{value}'")]
    TimestampInFuture { field: String, value: String },

    #[error("Challenge expirado: emitido en '{issued_at}', maximo {max_age_seconds}s")]
    ChallengeExpired {
        issued_at: String,
        max_age_seconds: u64,
    },

    #[error("Proof expirado: expiration_time '{expiration_time}' ya paso")]
    ProofExpired { expiration_time: String },

    #[error("Proof aun no es valido: not_before '{not_before}' esta en el futuro")]
    NotYetValid { not_before: String },

    #[error("Cadena no soportada: '{chain_id}'")]
    UnsupportedChain { chain_id: String },

    #[error("URI no coincide con el dominio: esperado dominio '{expected_domain}', URI recibida '{received_uri}'")]
    UriMismatch {
        expected_domain: String,
        received_uri: String,
    },

    // --- Errores de nonce ---

    #[error("Nonce invalido o ya utilizado: '{nonce}'")]
    NonceInvalid { nonce: String },

    #[error("Nonce expirado: '{nonce}'")]
    NonceExpired { nonce: String },

    // --- Errores de firma ---

    #[error("Formato de firma invalido: {reason}")]
    InvalidSignatureFormat { reason: String },

    #[error("Direccion invalida: {reason}")]
    InvalidAddress { reason: String },

    #[error("Verificacion de firma fallida: {reason}")]
    SignatureVerificationFailed { reason: String },

    #[error("Direccion no coincide: esperada '{expected}', recuperada '{recovered}'")]
    AddressMismatch { expected: String, recovered: String },

    // --- Errores internos ---

    #[error("Error interno del store de nonces: {reason}")]
    NonceStoreError { reason: String },
}
```

---

## 14. Estimacion de Esfuerzo

### 14.1 Desglose por Archivo

| Archivo | Lineas Estimadas | Complejidad | Descripcion |
|---|---|---|---|
| `siwx/mod.rs` | ~30 | Baja | Re-exports, SiwxState |
| `siwx/types.rs` | ~200 | Baja | Structs, enums, SiwxConfig con builders |
| `siwx/message.rs` | ~150 | Media | Construccion de mensajes SIWE/SIWS (debe ser byte-perfect) |
| `siwx/verify.rs` | ~250 | Alta | Verificacion EIP-191 + Ed25519 + validacion de campos |
| `siwx/nonce.rs` | ~120 | Media | NonceStore trait + InMemoryNonceStore con cleanup |
| `siwx/session.rs` | ~100 | Media | PaidAddressCache trait + InMemoryPaidAddressCache |
| `siwx/middleware.rs` | ~150 | Alta | Header parsing + integracion con X402Middleware |
| `siwx/error.rs` | ~60 | Baja | Enum SiwxError con thiserror |
| `layer.rs` (cambios) | ~80 | Media | Extension with_siwx + campos nuevos + flujo modificado |
| **x402-reqwest/siwx.rs** | ~100 | Media | Logica del cliente (firmar challenge, construir header) |
| **Tests unitarios** | ~400 | Media | Vectores de prueba, mocks, tests de integracion |
| **Cambios a Cargo.toml** | ~15 | Baja | Dependencias nuevas + feature flags |
| **TOTAL** | **~1,655** | **Media-Alta** | |

### 14.2 Esfuerzo por Fase

| Fase | Duracion Estimada | Prerrequisitos |
|---|---|---|
| **Fase 1: Tipos** | 1 dia | Ninguno |
| **Fase 2: Mensajes** | 1-2 dias | Fase 1. Requiere vectores de prueba SIWE/SIWS. |
| **Fase 3: Verificacion** | 2-3 dias | Fase 2. Es la parte mas critica y compleja. |
| **Fase 4: Nonces** | 0.5 dias | Fase 1 |
| **Fase 5: Sesiones** | 0.5 dias | Fase 1 |
| **Fase 6: Middleware** | 2-3 dias | Fases 1-5. Integracion con X402Middleware existente. |
| **Fase 7: Cliente** | 1-2 dias | Fases 1-2. Puede paralelizarse con Fase 3. |
| **Tests + QA** | 2-3 dias | Todas las fases |
| **TOTAL** | **10-15 dias** | |

### 14.3 Riesgo de la Estimacion

- **Riesgo principal**: La construccion de mensajes SIWE/SIWS debe ser byte-perfect. Un error sutil (un `\n` de mas, un espacio faltante) hara que todas las firmas fallen. Esto puede requerir iteracion significativa.
- **Mitigacion**: Usar vectores de prueba generados por implementaciones de referencia (siwe-rs, @spruceid/siwe-parser de JavaScript).
- **Alternativa**: Usar la crate `siwe` directamente para EVM y evitar reimplementar el parser. Esto reduce el riesgo de la Fase 2-3 significativamente pero agrega una dependencia.

---

## 15. Checklist de Verificacion

### 15.1 Tests Unitarios (siwx/tests/)

- [ ] **types**: Serializar/deserializar `SiwxChallengeInfo`, `SiwxProof`, `SiwxExtension` a/desde JSON
- [ ] **types**: Validar que los campos `rename_all = "camelCase"` producen JSON correcto
- [ ] **message SIWE**: Construir mensaje SIWE con todos los campos y comparar byte-por-byte con vector de referencia
- [ ] **message SIWE**: Construir mensaje SIWE sin campos opcionales
- [ ] **message SIWE**: Construir mensaje SIWE con multiples resources
- [ ] **message SIWS**: Construir mensaje SIWS y comparar byte-por-byte con vector de referencia
- [ ] **verify EIP-191**: Verificar firma valida de EOA (vector de prueba conocido)
- [ ] **verify EIP-191**: Rechazar firma con address incorrecto
- [ ] **verify EIP-191**: Rechazar firma con mensaje alterado
- [ ] **verify EIP-191**: Rechazar firma con formato hex invalido
- [ ] **verify EIP-191**: Rechazar firma con longitud incorrecta
- [ ] **verify Ed25519**: Verificar firma valida de Solana (vector de prueba conocido)
- [ ] **verify Ed25519**: Rechazar firma con clave publica incorrecta
- [ ] **verify Ed25519**: Rechazar firma con Base58 invalido
- [ ] **validate fields**: Rechazar domain que no coincide
- [ ] **validate fields**: Rechazar version != "1"
- [ ] **validate fields**: Rechazar issuedAt en el futuro (>30s)
- [ ] **validate fields**: Rechazar issuedAt demasiado antiguo
- [ ] **validate fields**: Rechazar expirationTime en el pasado
- [ ] **validate fields**: Rechazar notBefore en el futuro
- [ ] **validate fields**: Rechazar chainId no soportado
- [ ] **nonce**: Generar nonce y validar formato (32 hex chars)
- [ ] **nonce**: Generar y consumir nonce exitosamente
- [ ] **nonce**: Rechazar nonce no emitido
- [ ] **nonce**: Rechazar nonce ya consumido (replay)
- [ ] **nonce**: Rechazar nonce expirado
- [ ] **session**: Marcar direccion como pagada y verificar
- [ ] **session**: Verificar que direccion no pagada retorna false
- [ ] **session**: Verificar que entrada expirada retorna false
- [ ] **session**: Verificar case-insensitivity de direcciones
- [ ] **header parsing**: Decodificar header Base64 valido
- [ ] **header parsing**: Retornar None si header ausente
- [ ] **header parsing**: Retornar error si Base64 invalido
- [ ] **header parsing**: Retornar error si JSON invalido

### 15.2 Tests de Integracion

- [ ] **Flujo completo EVM**: 402 -> pago -> SIWX sign-in -> acceso sin pago
- [ ] **Flujo completo Solana**: 402 -> pago -> SIWX sign-in -> acceso sin pago
- [ ] **Multi-chain**: 402 con multiples cadenas soportadas -> autenticacion desde cualquiera
- [ ] **Expiracion de sesion**: Pagar -> esperar TTL -> SIWX rechazado -> requiere nuevo pago
- [ ] **Sin SIWX habilitado**: Verificar que el middleware funciona identico al actual sin el feature flag

### 15.3 Tests de Seguridad

- [ ] **Replay attack**: Usar el mismo proof SIWX dos veces -> segundo debe fallar (nonce consumido)
- [ ] **Cross-domain**: Usar proof firmado para dominio A en servidor de dominio B -> debe fallar
- [ ] **Expired proof**: Usar proof con issuedAt de hace >5 minutos -> debe fallar
- [ ] **Tampered message**: Modificar un campo del proof despues de firmar -> verificacion debe fallar
- [ ] **Wrong chain signature**: Enviar firma EVM en proof de Solana -> debe fallar

### 15.4 Verificacion en Produccion

- [ ] Compilar con `--features siwx-full` exitosamente
- [ ] Compilar sin `--features siwx` y verificar que todo sigue igual (backward compatible)
- [ ] Ejecutar el ejemplo `x402-axum-example` con SIWX habilitado
- [ ] Verificar que la respuesta 402 incluye `extensions.sign-in-with-x` cuando SIWX esta habilitado
- [ ] Verificar que el header `SIGN-IN-WITH-X` se procesa correctamente
- [ ] Verificar logs/telemetria: `tracing::info!` en verificacion exitosa, `tracing::warn!` en rechazos

---

## Apendice A: Estructura de Archivos Propuesta

```
crates/x402-axum/
  Cargo.toml                     # +chrono, +hex, +base64, +dashmap, +alloy, +ed25519-dalek, +bs58
  src/
    lib.rs                       # +pub mod siwx (behind #[cfg(feature = "siwx")])
    layer.rs                     # +campo siwx en X402Middleware
                                 # +metodo with_siwx()
                                 # +flujo SIWX en handle_request()
    siwx/
      mod.rs                     # Re-exports publicos, SiwxState
      types.rs                   # SiwxChallengeInfo, SiwxProof, SiwxSupportedChain,
                                 #   SiwxSignatureType, SiwxExtension, SiwxConfig
      message.rs                 # build_siwe_message(), build_siws_message()
      verify.rs                  # verify_siwx_signature(), verify_eip191(), verify_ed25519(),
                                 #   validate_siwx_fields(), SiwxVerifiedIdentity
      nonce.rs                   # NonceStore trait, InMemoryNonceStore
      session.rs                 # PaidAddressCache trait, InMemoryPaidAddressCache
      middleware.rs              # SIWX_HEADER_NAME, extract_siwx_proof(), encode_siwx_header()
      error.rs                   # SiwxError enum
    facilitator_client.rs        # (sin cambios)
    price.rs                     # (sin cambios)

crates/x402-reqwest/
  src/
    siwx.rs                      # SiwxClientSigner trait, encode_siwx_header(),
                                 #   sign_evm_challenge(), sign_solana_challenge()
    lib.rs                       # +pub mod siwx (behind feature flag)
    middleware.rs                 # +deteccion de extension SIWX en respuestas 402

examples/
  x402-axum-example/
    src/main.rs                  # +ejemplo de ruta protegida con SIWX habilitado
```

## Apendice B: Compatibilidad con Upstream

### Estado Actual del Upstream

El upstream (x402-rs/x402-rs v1.1.3) ha definido el spec SIWX pero **no tiene implementacion aun** en el codigo Rust. La spec vive en `docs/specs/extensions/sign-in-with-x.md` y define el protocolo a nivel de JSON/HTTP.

### Estrategia de Merge

Cuando el upstream implemente SIWX en codigo:

1. **Si implementan en x402-axum**: Comparar con nuestra implementacion. Adoptar lo que sea mejor, preservar nuestros tipos si son compatibles.
2. **Si implementan en un crate separado**: Evaluar si migrar o mantener nuestra implementacion.
3. **Si los tipos son incompatibles**: Usar nuestros wrappers de conversion (como ya hacemos con v1->v2 en `types_v2.rs`).

### Ventajas de Implementar Primero

- Nos da experiencia practica con el protocolo.
- Podemos contribuir feedback al upstream sobre problemas encontrados.
- Nuestros usuarios (servidores de recursos) pueden empezar a usar SIWX inmediatamente.
- Si el upstream adopta nuestra implementacion, reducimos el trabajo de merge.

## Apendice C: Relacion con Otros Componentes Existentes

### Reutilizacion de Codigo Existente

| Componente Existente | Uso en SIWX |
|---|---|
| `src/caip2.rs` (Caip2NetworkId, Namespace) | Validar los `chainId` en el proof SIWX. Ya soportamos `eip155:*` y `solana:*`. |
| `src/types_v2.rs` (PaymentPayloadV2.extensions) | El campo `extensions: HashMap<String, serde_json::Value>` ya existe y es donde se serializa la extension SIWX en la respuesta 402. |
| `src/types_v2.rs` (SupportedPaymentKindsResponseV2.extensions) | La respuesta de `/supported` ya tiene un campo para listar extensiones soportadas. Agregar `"sign_in_with_x"` cuando el feature flag esta habilitado. |
| `dashmap` (ya en Cargo.toml) | Para `InMemoryNonceStore` y `InMemoryPaidAddressCache`. |
| `alloy` (ya en Cargo.toml) | Para verificacion EIP-191 (ECDSA recovery, keccak256). |
| `ed25519-dalek` (ya en Cargo.toml) | Para verificacion Ed25519 (Solana). |
| `bs58` (ya en Cargo.toml) | Para decodificar direcciones y firmas Solana (Base58). |
| `base64` (ya en Cargo.toml) | Para codificar/decodificar el header HTTP. |

### Lo que NO se Reutiliza

| Funcionalidad | Razon |
|---|---|
| Verificacion EIP-712 (de `src/chain/evm.rs`) | SIWX usa EIP-191 (personal_sign), NO EIP-712 (typed data). Son mecanismos de firma completamente distintos. |
| Settlement logic (de `src/facilitator_local.rs`) | SIWX no involucra transacciones on-chain. Es solo verificacion de firma off-chain. |
| Provider cache (de `src/provider_cache.rs`) | SIWX no necesita RPC providers (excepto para futuro soporte EIP-1271 de smart contract wallets). |

---

*Fin del documento de implementacion. Ultima actualizacion: 2026-02-12.*
