# Reporte de Analisis: x402 Protocol v2 - Impacto en x402-rs Facilitator

**Fecha de analisis:** 2024-12-11
**Fuente:** https://github.com/coinbase/x402 (lanzado hoy)
**Analizado por:** Claude + Gemini CLI

---

## Resumen Ejecutivo

x402 v2 fue lanzado hoy por Coinbase y representa una evolucion significativa del protocolo de pagos HTTP 402. Los cambios son **BREAKING** y requieren accion de nuestra parte como facilitadores. Sin embargo, el impacto puede ser manejado de forma gradual.

---

## 1. Cambios Criticos (Breaking Changes)

### 1.1 Network Identifiers: De strings a CAIP-2

| Aspecto | v1 (Nuestro codigo actual) | v2 (Nuevo) |
|---------|---------------------------|------------|
| Base Sepolia | `"base-sepolia"` | `"eip155:84532"` |
| Base Mainnet | `"base"` | `"eip155:8453"` |
| Avalanche | `"avalanche"` | `"eip155:43114"` |
| Solana Mainnet | `"solana"` | `"solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"` |
| Solana Devnet | `"solana-devnet"` | `"solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1"` |

**Impacto en x402-rs:**
- `src/network.rs` lineas 20-114: El enum `Network` usa strings como `"base-sepolia"`
- La serializacion serde usa `#[serde(rename = "base-sepolia")]`
- Esto es **INCOMPATIBLE** con clientes v2

**CAIP-2 Format:** `{namespace}:{reference}`
- EVM chains: `eip155:{chainId}`
- Solana: `solana:{genesisHash}`

### 1.2 HTTP Headers Renombrados

| v1 (Actual) | v2 (Nuevo) |
|-------------|------------|
| `X-PAYMENT` | `PAYMENT-SIGNATURE` |
| `X-PAYMENT-RESPONSE` | `PAYMENT-RESPONSE` |
| Body JSON para 402 | `PAYMENT-REQUIRED` header (base64) |

**Impacto:**
- Los clientes v2 enviaran `PAYMENT-SIGNATURE`, no `X-PAYMENT`
- Nuestro facilitator no lee estos headers (solo expone `/verify` y `/settle`)
- **BAJO IMPACTO** para nosotros como facilitator puro

### 1.3 Estructura de PaymentPayload Completamente Nueva

**v1 (Nuestro codigo - `src/types.rs:325-330`):**
```rust
pub struct PaymentPayload {
    pub x402_version: X402Version,
    pub scheme: Scheme,
    pub network: Network,
    pub payload: ExactPaymentPayload,
}
```

**v2 (Nuevo):**
```json
{
  "x402Version": 2,
  "resource": {
    "url": "https://api.example.com/premium-data",
    "description": "Access to premium market data",
    "mimeType": "application/json"
  },
  "accepted": {
    "scheme": "exact",
    "network": "eip155:84532",
    "amount": "10000",
    "asset": "0x...",
    "payTo": "0x...",
    "maxTimeoutSeconds": 60,
    "extra": {...}
  },
  "payload": {...},
  "extensions": {}
}
```

**Cambios clave:**
- Nuevo campo `resource: ResourceInfo` (obligatorio)
- `scheme` y `network` movidos a `accepted`
- Nuevo campo `accepted: PaymentRequirements`
- Nuevo campo `extensions: Record<string, unknown>`
- Campo `amount` en vez de `maxAmountRequired`

### 1.4 PaymentRequirements Reestructurado

**v1 (Nuestro codigo - `src/types.rs:928-943`):**
```rust
pub struct PaymentRequirements {
    pub scheme: Scheme,
    pub network: Network,
    pub max_amount_required: TokenAmount,  // DEPRECATED in v2
    pub resource: Url,  // MOVED to top level in v2
    pub description: String,  // MOVED to ResourceInfo
    pub mime_type: String,  // MOVED to ResourceInfo
    pub output_schema: Option<serde_json::Value>,  // REMOVED in v2
    pub pay_to: MixedAddress,
    pub max_timeout_seconds: u64,
    pub asset: MixedAddress,
    pub extra: Option<serde_json::Value>,
}
```

**v2:**
```typescript
type PaymentRequirements = {
  scheme: string;
  network: Network;  // CAIP-2 format
  asset: string;
  amount: string;  // Renamed from maxAmountRequired
  payTo: string;
  maxTimeoutSeconds: number;
  extra: Record<string, unknown>;
};
```

**Campos eliminados en v2:**
- `resource` (movido a nivel superior)
- `description` (movido a ResourceInfo)
- `mimeType` (movido a ResourceInfo)
- `outputSchema` (eliminado)

**Campos renombrados:**
- `maxAmountRequired` -> `amount`

---

## 2. Nuevas Features en v2

### 2.1 Sistema de Extensions
```json
{
  "extensions": {
    "bazaar": {
      "info": {...},
      "schema": {...}
    },
    "sign_in_with_x": {
      "info": {...},
      "schema": {...}
    }
  }
}
```

- Framework modular para agregar features opcionales
- Permite sign-in with Ethereum, discovery, etc.
- **Opcional** - no impacta compatibilidad basica

### 2.2 Discovery API (Bazaar)

Nuevo endpoint `GET /discovery/resources`:
```json
{
  "x402Version": 2,
  "items": [
    {
      "resource": "https://api.example.com/premium-data",
      "type": "http",
      "x402Version": 1,
      "accepts": [...],
      "lastUpdated": 1703123456,
      "metadata": {
        "category": "finance",
        "provider": "Example Corp"
      }
    }
  ],
  "pagination": {
    "limit": 10,
    "offset": 0,
    "total": 1
  }
}
```

- Marketplace para descubrir recursos x402
- **No tenemos** - podriamos agregarlo como feature

### 2.3 `/supported` Response Expandido

**v2 agrega:**
```json
{
  "kinds": [
    {
      "x402Version": 2,
      "scheme": "exact",
      "network": "eip155:84532"
    }
  ],
  "extensions": ["bazaar", "sign_in_with_x"],
  "signers": {
    "eip155:*": ["0x1234..."],
    "solana:*": ["CKPKJWNd..."]
  }
}
```

**Nuestro codigo actual (`src/types.rs:1510-1515`):**
```rust
pub struct SupportedPaymentKindsResponse {
    pub kinds: Vec<SupportedPaymentKind>,
}
```

**Campos faltantes:**
- `extensions: Vec<String>`
- `signers: HashMap<String, Vec<String>>`

### 2.4 ResourceInfo (Nuevo tipo)
```typescript
interface ResourceInfo {
  url: string;
  description: string;
  mimeType: string;
}
```

Este tipo extrae la informacion del recurso que antes estaba duplicada en cada PaymentRequirements.

---

## 3. Multi-Transport Support

v2 define transportes formalmente:

### 3.1 Transportes Soportados
| Transporte | Archivo Spec | Uso |
|------------|--------------|-----|
| HTTP | `specs/transports-v2/http.md` | Web APIs tradicionales |
| MCP | `specs/transports-v2/mcp.md` | Model Context Protocol (AI agents) |
| A2A | `specs/transports-v2/a2a.md` | Agent-to-Agent Protocol |

### 3.2 MCP (Model Context Protocol)
- Permite a AI agents pagar por herramientas y recursos
- Usa JSON-RPC con error code 402
- Muy relevante para integracion con Claude, GPT, etc.

### 3.3 A2A (Agent-to-Agent)
- Pagos directos entre agentes autonomos
- Task-based state management
- Metadata system para coordinar pagos

---

## 4. Multi-Chain Support

### 4.1 Chains soportados oficialmente en v2:
- **EVM (eip155:*):** Base, Avalanche, Optimism, Polygon, Arbitrum, Ethereum
- **Solana (solana:*):** Mainnet y Devnet
- **Sui:** Nuevo spec (`scheme_exact_sui.md`)

### 4.2 Nuestra ventaja competitiva:
Tenemos soporte para chains que upstream **NO** tiene:

| Chain | Nosotros | Upstream v2 |
|-------|----------|-------------|
| Stellar | Si | No |
| NEAR | Si | No |
| Fogo | Si | No |
| HyperEVM | Si | No |
| Celo | Si | No |
| Sei | Si | No |
| Unichain | Si | No |
| Monad | Si | No |
| Sui | No | Si |

### 4.3 Consideracion Sui
Upstream agrego soporte para Sui. Deberiamos considerar agregarlo en el futuro.

---

## 5. Backward Compatibility

### 5.1 Puede un facilitator v2 soportar v1?
**Si, pero requiere codigo explicito:**

```typescript
// TypeScript SDK approach
const facilitator = new x402Facilitator();

// Register v2 schemes
facilitator.register(["eip155:8453", "eip155:84532"], evmFacilitator);

// Register v1 schemes (backward compat)
facilitator.registerV1(["base", "base-sepolia"], evmFacilitatorV1);
```

- Hay paquete `@x402/legacy` para tipos v1
- El facilitator debe detectar `x402Version` y rutear

### 5.2 Path de migracion recomendado:
1. **Fase 1:** Agregar soporte para v2 manteniendo v1
2. **Fase 2:** Deprecar v1 despues de 6 meses
3. **Fase 3:** Remover v1

### 5.3 Como detectar version:
```rust
// En el request
match request.x402_version {
    1 => handle_v1(request),
    2 => handle_v2(request),
    _ => Err("Unsupported version"),
}
```

---

## 6. Impacto Especifico para x402-rs

### 6.1 Archivos que requieren cambios

| Archivo | Cambio Requerido | Prioridad | Estimacion |
|---------|------------------|-----------|------------|
| `src/types.rs` | Agregar tipos v2 (ResourceInfo, PaymentPayloadV2, etc.) | **ALTA** | 4-6 horas |
| `src/network.rs` | Agregar mapeo Network <-> CAIP-2 | **ALTA** | 2-3 horas |
| `src/handlers.rs` | Detectar version y rutear | **MEDIA** | 2-4 horas |
| `src/types.rs:1510` | Expandir SupportedPaymentKindsResponse | **MEDIA** | 1-2 horas |
| Tests | Agregar tests para v2 | **MEDIA** | 4-6 horas |

### 6.2 Cambios especificos en types.rs

**Nuevos tipos a agregar:**
```rust
/// v2 ResourceInfo
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceInfo {
    pub url: Url,
    pub description: String,
    pub mime_type: String,
}

/// v2 PaymentRequirements (simplificado)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequirementsV2 {
    pub scheme: Scheme,
    pub network: String,  // CAIP-2 format
    pub amount: TokenAmount,
    pub asset: MixedAddress,
    pub pay_to: MixedAddress,
    pub max_timeout_seconds: u64,
    pub extra: Option<serde_json::Value>,
}

/// v2 PaymentPayload
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentPayloadV2 {
    pub x402_version: u8,  // Always 2
    pub resource: ResourceInfo,
    pub accepted: PaymentRequirementsV2,
    pub payload: ExactPaymentPayload,
    #[serde(default)]
    pub extensions: HashMap<String, serde_json::Value>,
}
```

### 6.3 Cambios en network.rs

**Agregar mapeo CAIP-2:**
```rust
impl Network {
    /// Convert to CAIP-2 format for v2 compatibility
    pub fn to_caip2(&self) -> String {
        match self {
            Network::Base => "eip155:8453".to_string(),
            Network::BaseSepolia => "eip155:84532".to_string(),
            Network::Avalanche => "eip155:43114".to_string(),
            Network::Solana => "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            // ... etc
        }
    }

    /// Parse from CAIP-2 format
    pub fn from_caip2(s: &str) -> Result<Self, NetworkParseError> {
        match s {
            "eip155:8453" => Ok(Network::Base),
            "eip155:84532" => Ok(Network::BaseSepolia),
            // ... etc
        }
    }
}
```

### 6.4 Estimacion de trabajo total

**Opcion A: Soporte completo v1+v2**
- ~3-5 dias de desarrollo
- Complejidad: Media-Alta
- Mantiene compatibilidad total

**Opcion B: Migracion directa a v2**
- ~2-3 dias de desarrollo
- Rompe clientes v1 existentes
- Mas simple de mantener

### 6.5 Riesgo de no actualizar
| Plazo | Riesgo | Razon |
|-------|--------|-------|
| Corto (1-3 meses) | Bajo | Mayoria de clientes siguen en v1 |
| Mediano (3-6 meses) | Medio | Nuevos clientes usaran v2 |
| Largo (6+ meses) | Alto | v1 sera deprecated |

---

## 7. Analisis de Gemini (Segunda Opinion)

Gemini identifico los mismos puntos criticos y agrego:

> "La arquitectura v2 separa claramente Types, Logic, y Representation. Esto hace el protocolo mas extensible pero requiere mas codigo en el facilitator para manejar la abstraccion."

> "El sistema de Extensions es la feature mas importante para el futuro - permite agregar funcionalidad sin romper el protocolo base."

> "La migracion de network strings a CAIP-2 es el cambio mas impactante. Sin embargo, permite interoperabilidad con otros protocolos que usan el estandar CAIP."

---

## 8. Recomendaciones

### Accion Inmediata (Esta semana):
1. **NO romper** el codigo actual
2. Crear branch `feature/x402-v2-support`
3. Documentar mapeo completo Network <-> CAIP-2

### Accion a Corto Plazo (2-4 semanas):
1. Implementar parsing CAIP-2 <-> Network
2. Agregar tipos v2 en `types.rs`
3. Actualizar `/supported` con `extensions` y `signers`
4. Tests de compatibilidad v1/v2

### Accion a Mediano Plazo (1-3 meses):
1. Implementar Discovery API (Bazaar)
2. Sistema de Extensions basico
3. Documentar migracion para nuestros usuarios
4. Considerar soporte Sui

### Consideraciones de AI/MCP:
- MCP transport es muy relevante para integracion con AI agents
- Podria ser diferenciador competitivo
- Evaluar prioridad basado en demanda del mercado

---

## 9. Referencias

- **Spec v2:** https://github.com/coinbase/x402/blob/main/specs/x402-specification-v2.md
- **Spec v1:** https://github.com/coinbase/x402/blob/main/specs/x402-specification-v1.md
- **HTTP Transport v2:** https://github.com/coinbase/x402/blob/main/specs/transports-v2/http.md
- **MCP Transport:** https://github.com/coinbase/x402/blob/main/specs/transports-v2/mcp.md
- **TypeScript SDK:** https://github.com/coinbase/x402/tree/main/typescript
- **CAIP-2 Standard:** https://github.com/ChainAgnostic/CAIPs/blob/main/CAIPs/caip-2.md

---

## 10. Conclusion

x402 v2 es una evolucion necesaria que mejora la interoperabilidad y extensibilidad. Los cambios son breaking pero manejables. **Nuestra ventaja** es que tenemos soporte para chains que upstream no tiene (Stellar, NEAR, Fogo, etc.).

**Recomendacion final:** Implementar soporte dual v1+v2 en las proximas 2-4 semanas, posicionando a Ultravioleta DAO como un facilitator que soporta tanto la base instalada (v1) como el futuro (v2).

---

## Apendice A: Mapeo Completo Network <-> CAIP-2

| Network Enum | v1 String | v2 CAIP-2 | Chain ID |
|--------------|-----------|-----------|----------|
| Base | `base` | `eip155:8453` | 8453 |
| BaseSepolia | `base-sepolia` | `eip155:84532` | 84532 |
| Avalanche | `avalanche` | `eip155:43114` | 43114 |
| AvalancheFuji | `avalanche-fuji` | `eip155:43113` | 43113 |
| Polygon | `polygon` | `eip155:137` | 137 |
| PolygonAmoy | `polygon-amoy` | `eip155:80002` | 80002 |
| Optimism | `optimism` | `eip155:10` | 10 |
| OptimismSepolia | `optimism-sepolia` | `eip155:11155420` | 11155420 |
| Ethereum | `ethereum` | `eip155:1` | 1 |
| EthereumSepolia | `ethereum-sepolia` | `eip155:11155111` | 11155111 |
| Arbitrum | `arbitrum` | `eip155:42161` | 42161 |
| ArbitrumSepolia | `arbitrum-sepolia` | `eip155:421614` | 421614 |
| Celo | `celo` | `eip155:42220` | 42220 |
| CeloSepolia | `celo-sepolia` | `eip155:44787` | 44787 |
| HyperEvm | `hyperevm` | `eip155:999` | 999 |
| HyperEvmTestnet | `hyperevm-testnet` | `eip155:333` | 333 |
| Sei | `sei` | `eip155:1329` | 1329 |
| SeiTestnet | `sei-testnet` | `eip155:1328` | 1328 |
| Unichain | `unichain` | `eip155:130` | 130 |
| UnichainSepolia | `unichain-sepolia` | `eip155:1301` | 1301 |
| Monad | `monad` | `eip155:143` | 143 |
| XdcMainnet | `xdc` | `eip155:50` | 50 |
| XrplEvm | `xrpl-evm` | `eip155:1440000` | 1440000 |
| Solana | `solana` | `solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp` | N/A |
| SolanaDevnet | `solana-devnet` | `solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1` | N/A |
| Near | `near` | `near:mainnet` | N/A |
| NearTestnet | `near-testnet` | `near:testnet` | N/A |
| Stellar | `stellar` | `stellar:pubnet` | N/A |
| StellarTestnet | `stellar-testnet` | `stellar:testnet` | N/A |
| Fogo | `fogo` | `fogo:mainnet` (TBD) | N/A |
| FogoTestnet | `fogo-testnet` | `fogo:testnet` (TBD) | N/A |

**Nota:** Los CAIP-2 para NEAR, Stellar, y Fogo no estan definidos oficialmente. Usamos convenciones razonables pero debemos verificar si hay estandares establecidos.
