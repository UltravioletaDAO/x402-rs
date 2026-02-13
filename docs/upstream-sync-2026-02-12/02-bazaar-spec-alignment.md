# 02 - Alineacion con la Especificacion Bazaar del Upstream

**Fecha**: 2026-02-12
**Autor**: Claude Opus 4.6 (analisis automatizado)
**Version del fork**: v1.32.1
**Version upstream**: v1.1.3
**Spec analizada**: `upstream/main:docs/specs/extensions/bazaar.md` (descargada 2026-02-03)

---

## Resumen de la Funcionalidad

### Que define la especificacion Bazaar del upstream

La especificacion Bazaar (`docs/specs/extensions/bazaar.md`) define un **mecanismo de descubrimiento y catalogacion de recursos** para endpoints habilitados con x402. Es una **extension del protocolo** que viaja dentro del campo `extensions` de las respuestas/payloads x402 v2.

**Modelo conceptual del spec upstream:**

1. Un **Resource Server** declara sus endpoints en la respuesta `402 Payment Required` usando la extension `bazaar` dentro del objeto `extensions`.
2. La extension tiene dos campos: `info` (datos de descubrimiento) y `schema` (JSON Schema Draft 2020-12 para validar `info`).
3. El **Facilitator** recibe el payload con la extension bazaar, valida `info` contra `schema`, y cataloga el recurso.
4. El **Client** debe copiar la extension bazaar del `PaymentRequired` a su `PaymentPayload`.
5. El almacenamiento e indexacion es un detalle de implementacion del facilitator.

**Estructura clave del spec:**

```json
{
  "extensions": {
    "bazaar": {
      "info": {
        "input": {
          "type": "http",
          "method": "GET|POST|PUT|PATCH|DELETE|HEAD",
          "queryParams": {},       // Para GET/HEAD/DELETE
          "bodyType": "json",      // Para POST/PUT/PATCH
          "body": {},              // Para POST/PUT/PATCH
          "headers": {}            // Opcional
        },
        "output": {
          "type": "json|text",
          "format": "...",         // Opcional
          "example": {}            // Opcional
        }
      },
      "schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": { ... },
        "required": ["input"]
      }
    }
  }
}
```

---

## Nuestra Implementacion Actual

### Arquitectura Meta-Bazaar

Nuestra implementacion es significativamente **mas ambiciosa** que el spec upstream. Hemos construido un sistema completo de "Meta-Bazaar" que va mas alla de lo que el spec define. Nuestra arquitectura tiene **4 fuentes de descubrimiento**, un **registro persistente**, y un **sistema de agregacion multi-facilitador**.

### Archivos y componentes

| Archivo | Lineas | Funcion |
|---------|--------|---------|
| `/mnt/z/ultravioleta/dao/x402-rs/src/discovery.rs` | 923 | Registro principal con cache en memoria + almacenamiento persistente |
| `/mnt/z/ultravioleta/dao/x402-rs/src/discovery_aggregator.rs` | 1067 | Agregador que recolecta de 12 facilitadores externos |
| `/mnt/z/ultravioleta/dao/x402-rs/src/discovery_crawler.rs` | 483 | Crawler de endpoints `/.well-known/x402` |
| `/mnt/z/ultravioleta/dao/x402-rs/src/discovery_store.rs` | 469 | Abstraccion de almacenamiento (S3, Memoria, NoOp) |
| `/mnt/z/ultravioleta/dao/x402-rs/src/types_v2.rs` | Lineas 925-1295 | Tipos: `DiscoveryResource`, `DiscoverySource`, `DiscoveryFilters`, etc. |
| `/mnt/z/ultravioleta/dao/x402-rs/src/handlers.rs` | Lineas 150-320, 1070-1539 | Endpoints HTTP + integracion settlement |

### Fuentes de descubrimiento (enum `DiscoverySource`)

Definido en `/mnt/z/ultravioleta/dao/x402-rs/src/types_v2.rs`, lineas 937-962:

```rust
pub enum DiscoverySource {
    SelfRegistered,  // POST /discovery/register
    Settlement,      // Auto-registro via /settle con discoverable=true
    Crawled,         // Descubierto via /.well-known/x402
    Aggregated,      // Importado de otro facilitador
}
```

### Endpoints expuestos

| Endpoint | Metodo | Descripcion |
|----------|--------|-------------|
| `GET /discovery/resources` | GET | Lista recursos con paginacion y filtros |
| `POST /discovery/register` | POST | Registra un nuevo recurso |
| `GET /supported` | GET | Incluye `"bazaar"` en la lista de extensiones |

### Registro con persistencia

Definido en `/mnt/z/ultravioleta/dao/x402-rs/src/discovery.rs`, lineas 106-111:

```rust
pub struct DiscoveryRegistry {
    /// In-memory cache: Map of URL -> DiscoveryResource
    resources: Arc<RwLock<HashMap<String, DiscoveryResource>>>,
    /// Persistent storage backend
    store: Arc<dyn DiscoveryStore>,
}
```

### Estructura del recurso descubierto

Definido en `/mnt/z/ultravioleta/dao/x402-rs/src/types_v2.rs`, lineas 1005-1047:

```rust
pub struct DiscoveryResource {
    pub url: Url,
    pub resource_type: String,       // "http", "mcp", "a2a", "facilitator"
    pub x402_version: u8,
    pub description: String,
    pub accepts: Vec<PaymentRequirementsV2>,
    pub last_updated: u64,
    pub metadata: Option<DiscoveryMetadata>,
    // === Meta-Bazaar ===
    pub source: DiscoverySource,
    pub source_facilitator: Option<String>,
    pub first_seen: Option<u64>,
    pub settlement_count: Option<u32>,
}
```

### Agregacion multi-facilitador

Definido en `/mnt/z/ultravioleta/dao/x402-rs/src/discovery_aggregator.rs`, lineas 386-401:

```rust
pub fn all() -> Vec<Self> {
    vec![
        Self::coinbase(),     // api.cdp.coinbase.com
        Self::payai(),        // facilitator.payai.network
        Self::thirdweb(),     // api.thirdweb.com
        Self::questflow(),    // facilitator.questflow.ai
        Self::aurracloud(),   // x402-facilitator.aurracloud.com
        Self::anyspend(),     // mainnet.anyspend.com
        Self::openx402(),     // open.x402.host
        Self::x402rs(),       // facilitator.x402.rs
        Self::heurist(),      // facilitator.heurist.xyz
        Self::polymer(),      // api.polymer.zone
        Self::meridian(),     // api.mrdn.finance
        Self::virtuals(),     // acpx.virtuals.io
    ]
}
```

---

## Analisis de Brechas (Gap Analysis)

### Tabla comparativa punto por punto

| # | Requisito del Spec | Nuestra Implementacion | Estado | Impacto |
|---|-------------------|----------------------|--------|---------|
| 1 | Extension `bazaar` en `PaymentRequired` (402) con campos `info` y `schema` | **NO IMPLEMENTADO**. Nuestro sistema no genera respuestas 402 con la extension bazaar. No somos un Resource Server, somos un Facilitator. | N/A (no aplica a facilitadores) | Ninguno |
| 2 | Campo `info.input` con `type`, `method`, `queryParams`/`bodyType`/`body`, `headers` | **NO IMPLEMENTADO**. Nuestro `DiscoveryResource` no tiene campos `input`/`output` que describan como llamar al endpoint. | BRECHA MEDIA | Se pierde informacion sobre como consumir el endpoint |
| 3 | Campo `info.output` con `type`, `format`, `example` | **NO IMPLEMENTADO**. No almacenamos informacion de output del endpoint. | BRECHA MEDIA | Se pierde informacion de respuesta esperada |
| 4 | Campo `schema` (JSON Schema Draft 2020-12) para validar `info` | **NO IMPLEMENTADO**. No generamos ni validamos JSON Schemas. | BRECHA ALTA | El spec requiere validacion obligatoria |
| 5 | Facilitator DEBE validar `info` contra `schema` antes de catalogar | **NO IMPLEMENTADO**. No hay validacion de schema JSON en el flujo de catalogacion. | BRECHA ALTA | Violacion directa del spec |
| 6 | Client debe copiar extension bazaar de `PaymentRequired` a `PaymentPayload` | **PARCIAL**. `PaymentPayloadV2` tiene campo `extensions: HashMap<String, Value>` que puede transportar la extension. | ALINEADO (estructura existe) | Sin brecha funcional |
| 7 | Resource discovery y cataloging | **SUPERADO**. Nuestro sistema va mucho mas alla: 4 fuentes, persistencia S3, agregacion de 12 facilitadores, crawler, tracking de settlements. | SUPERADO | Nuestra implementacion es un superset |
| 8 | Facilitator almacena, indexa y expone recursos descubiertos | **IMPLEMENTADO**. `GET /discovery/resources` con filtros, paginacion, y multiples fuentes. | ALINEADO | Sin brecha |
| 9 | Extension `bazaar` listada en `/supported` | **IMPLEMENTADO**. `handlers.rs` linea 682: `vec!["bazaar".to_string()]` | ALINEADO | Sin brecha |
| 10 | Retrocompatibilidad v1 (`outputSchema` -> `extensions.bazaar`) | **NO IMPLEMENTADO**. No hacemos conversion de v1 `outputSchema` a `extensions.bazaar`. | BRECHA BAJA | Pocos clientes v1 existentes |

### Detalle de las brechas criticas

#### BRECHA 1: Falta de campos `info.input` y `info.output` en DiscoveryResource

**Donde**: `/mnt/z/ultravioleta/dao/x402-rs/src/types_v2.rs`, lineas 1005-1047

El spec define que cada recurso descubierto debe tener informacion sobre como llamar al endpoint (metodo HTTP, parametros, cuerpo) y que tipo de respuesta devuelve. Nuestro `DiscoveryResource` no tiene estos campos.

**Que falta especificamente:**

```rust
// Lo que el spec requiere y no tenemos:
pub struct BazaarInfo {
    pub input: BazaarInput,
    pub output: Option<BazaarOutput>,
}

pub struct BazaarInput {
    #[serde(rename = "type")]
    pub input_type: String,          // Siempre "http"
    pub method: String,              // "GET", "POST", etc.
    pub query_params: Option<Value>, // Para GET/HEAD/DELETE
    pub body_type: Option<String>,   // "json", "form-data", "text"
    pub body: Option<Value>,         // Para POST/PUT/PATCH
    pub headers: Option<Value>,      // Opcional
}

pub struct BazaarOutput {
    #[serde(rename = "type")]
    pub output_type: String,         // "json", "text", etc.
    pub format: Option<String>,
    pub example: Option<Value>,
}
```

#### BRECHA 2: Falta validacion JSON Schema

**Donde**: La validacion deberia ocurrir en `/mnt/z/ultravioleta/dao/x402-rs/src/handlers.rs`, linea 1466+ (flujo de settlement) y en cualquier punto donde se procese la extension bazaar del payload.

El spec es explicito:

> Facilitators **must** validate `info` against `schema` before cataloging.

Actualmente, cuando procesamos un settlement con `discoverable=true` (lineas 1466-1514 de `handlers.rs`), simplemente creamos un `DiscoveryResource` sin extraer ni validar la extension bazaar del payload:

```rust
// Codigo actual en handlers.rs:1480-1486
let discovery_resource = DiscoveryResource::from_settlement(
    body.payment_requirements.resource.clone(),
    "http".to_string(), // Default to HTTP resource type
    body.payment_requirements.description.clone(),
    vec![requirements_v2],
);
```

**Lo que deberia hacer segun el spec:**

1. Extraer `extensions.bazaar` del `PaymentPayloadV2`
2. Obtener `info` y `schema` de la extension
3. Validar `info` contra `schema` usando un validador JSON Schema Draft 2020-12
4. Si la validacion pasa, catalogar con los campos `input`/`output`
5. Si falla, rechazar la catalogacion (no el settlement)

#### BRECHA 3: Falta conversion v1 outputSchema

**Donde**: No existe codigo para esta conversion.

El spec define una tabla de mapeo:

| V1 Location | V2 Location |
|-------------|-------------|
| `accepts[0].outputSchema` | `extensions.bazaar` |
| `accepts[0].resource` | `resource.url` |
| `accepts[0].description` | `description` (top-level) |
| `accepts[0].mimeType` | `mimeType` (top-level) |

Aunque el spec dice que los facilitadores **no estan obligados** a soportar v1, seria util para agregar recursos de facilitadores que aun usan v1.

---

## Plan de Alineacion

### Fase 1: Tipos y estructuras (PRIORIDAD ALTA)

**Archivo**: `/mnt/z/ultravioleta/dao/x402-rs/src/types_v2.rs`

**Cambio 1.1**: Agregar tipos para la extension Bazaar

```rust
// Agregar despues de la linea 1047 (fin de DiscoveryResource)

/// Extension Bazaar segun el spec upstream.
///
/// Contiene informacion de descubrimiento (input/output) y el schema
/// JSON Schema Draft 2020-12 para validar la informacion.
///
/// Ref: upstream/main:docs/specs/extensions/bazaar.md
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BazaarExtension {
    /// Informacion de descubrimiento del endpoint
    pub info: BazaarInfo,
    /// JSON Schema (Draft 2020-12) que valida la estructura de `info`
    pub schema: serde_json::Value,
}

/// Informacion de descubrimiento de un endpoint x402.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BazaarInfo {
    /// Descripcion de como llamar al endpoint
    pub input: BazaarInput,
    /// Descripcion del formato de respuesta (opcional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<BazaarOutput>,
}

/// Como llamar a un endpoint protegido por x402.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BazaarInput {
    /// Siempre "http"
    #[serde(rename = "type")]
    pub input_type: String,

    /// Metodo HTTP: "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"
    pub method: String,

    /// Parametros de query (para GET/HEAD/DELETE)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_params: Option<serde_json::Value>,

    /// Tipo de cuerpo (para POST/PUT/PATCH): "json", "form-data", "text"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_type: Option<String>,

    /// Ejemplo de cuerpo de la solicitud (para POST/PUT/PATCH)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,

    /// Headers HTTP personalizados (opcional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<serde_json::Value>,
}

/// Formato de respuesta esperado de un endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BazaarOutput {
    /// Tipo de contenido: "json", "text", etc.
    #[serde(rename = "type")]
    pub output_type: String,

    /// Informacion adicional de formato
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// Ejemplo de respuesta
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
}
```

**Cambio 1.2**: Agregar campo `bazaar_info` a `DiscoveryResource`

```rust
// En DiscoveryResource (types_v2.rs, linea ~1047), agregar:
pub struct DiscoveryResource {
    // ... campos existentes ...

    /// Informacion de la extension Bazaar (input/output del endpoint)
    /// Presente cuando el recurso fue registrado con la extension bazaar
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bazaar_info: Option<BazaarInfo>,
}
```

### Fase 2: Validacion JSON Schema (PRIORIDAD ALTA)

**Dependencia nueva**: Crate `jsonschema` para validacion JSON Schema Draft 2020-12.

**Archivo nuevo**: `/mnt/z/ultravioleta/dao/x402-rs/src/bazaar_validator.rs`

```rust
//! Validacion de la extension Bazaar segun el spec upstream.
//!
//! Valida `info` contra `schema` usando JSON Schema Draft 2020-12,
//! como requiere la especificacion.

use serde_json::Value;

/// Resultado de validar una extension Bazaar.
#[derive(Debug)]
pub enum BazaarValidationResult {
    /// Validacion exitosa, info extraida
    Valid(crate::types_v2::BazaarInfo),
    /// Schema invalido
    InvalidSchema(String),
    /// Info no pasa la validacion del schema
    ValidationFailed(Vec<String>),
    /// Extension bazaar no presente en el payload
    NotPresent,
}

/// Extraer y validar la extension bazaar de un HashMap de extensiones.
pub fn validate_bazaar_extension(
    extensions: &std::collections::HashMap<String, Value>,
) -> BazaarValidationResult {
    let bazaar_value = match extensions.get("bazaar") {
        Some(v) => v,
        None => return BazaarValidationResult::NotPresent,
    };

    // Deserializar la extension
    let bazaar_ext: crate::types_v2::BazaarExtension = match serde_json::from_value(bazaar_value.clone()) {
        Ok(ext) => ext,
        Err(e) => return BazaarValidationResult::InvalidSchema(
            format!("Failed to parse bazaar extension: {}", e)
        ),
    };

    // Validar info contra schema usando jsonschema crate
    let validator = match jsonschema::validator_for(&bazaar_ext.schema) {
        Ok(v) => v,
        Err(e) => return BazaarValidationResult::InvalidSchema(
            format!("Invalid JSON Schema: {}", e)
        ),
    };

    let info_value = match serde_json::to_value(&bazaar_ext.info) {
        Ok(v) => v,
        Err(e) => return BazaarValidationResult::InvalidSchema(
            format!("Failed to serialize info: {}", e)
        ),
    };

    let errors: Vec<String> = validator
        .iter_errors(&info_value)
        .map(|e| e.to_string())
        .collect();

    if errors.is_empty() {
        BazaarValidationResult::Valid(bazaar_ext.info)
    } else {
        BazaarValidationResult::ValidationFailed(errors)
    }
}
```

### Fase 3: Integracion en el flujo de settlement (PRIORIDAD MEDIA)

**Archivo**: `/mnt/z/ultravioleta/dao/x402-rs/src/handlers.rs`, lineas 1466-1514

Modificar el bloque de settlement tracking para extraer la extension bazaar:

```rust
// Codigo propuesto para reemplazar handlers.rs lineas 1466-1514

// Phase 2: Settlement Tracking - check if discoverable=true
let is_discoverable = body
    .payment_requirements
    .extra
    .as_ref()
    .and_then(|e| e.get("discoverable"))
    .and_then(|v| v.as_bool())
    .unwrap_or(false);

if is_discoverable {
    use crate::types_v2::PaymentRequirementsV1ToV2;
    let (_resource_info, requirements_v2) = body.payment_requirements.to_v2();

    // Intentar extraer y validar la extension bazaar del payload
    let bazaar_info = {
        // El payload v2 puede tener extensions
        // Nota: body.payment_payload es v1, necesitamos verificar
        // si hay una version v2 con extensions
        // TODO: Implementar extraccion de extensions del payload
        None::<crate::types_v2::BazaarInfo>
    };

    let mut discovery_resource = DiscoveryResource::from_settlement(
        body.payment_requirements.resource.clone(),
        "http".to_string(),
        body.payment_requirements.description.clone(),
        vec![requirements_v2],
    );

    // Si tenemos bazaar info validada, agregarla al recurso
    discovery_resource.bazaar_info = bazaar_info;

    // Track the settlement (register or increment count)
    let registry = discovery_registry.clone();
    let resource_url = discovery_resource.url.to_string();
    tokio::spawn(async move {
        match registry.track_settlement(discovery_resource).await {
            // ... igual que antes ...
        }
    });
}
```

### Fase 4: Integracion en la API de registro (PRIORIDAD BAJA)

**Archivo**: `/mnt/z/ultravioleta/dao/x402-rs/src/types_v2.rs`

Agregar campo opcional `bazaar_info` a `RegisterResourceRequest`:

```rust
pub struct RegisterResourceRequest {
    pub url: Url,
    pub resource_type: String,
    pub description: String,
    pub accepts: Vec<PaymentRequirementsV2>,
    pub metadata: Option<DiscoveryMetadata>,
    // NUEVO: Informacion de la extension bazaar
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bazaar_info: Option<BazaarInfo>,
}
```

### Fase 5: Conversion v1 outputSchema (PRIORIDAD BAJA)

**Archivo**: `/mnt/z/ultravioleta/dao/x402-rs/src/discovery_aggregator.rs`

Agregar logica en `convert_single_resource()` para mapear el campo v1 `outputSchema` (si existe) a la estructura `BazaarInfo`:

```rust
// En convert_single_resource(), despues de la conversion de metadata:
// Si el recurso Coinbase tiene outputSchema, convertir a BazaarInfo
if let Some(output_schema) = cb.output_schema {
    resource.bazaar_info = Some(BazaarInfo {
        input: BazaarInput {
            input_type: "http".to_string(),
            method: "GET".to_string(), // Default para v1
            query_params: None,
            body_type: None,
            body: None,
            headers: None,
        },
        output: Some(BazaarOutput {
            output_type: "json".to_string(),
            format: None,
            example: Some(output_schema),
        }),
    });
}
```

---

## Cambios que Rompen Compatibilidad (Breaking Changes)

### Evaluacion de impacto

| Cambio | Rompe clientes existentes? | Detalle |
|--------|---------------------------|---------|
| Agregar `bazaar_info` a `DiscoveryResource` | **NO** | Campo opcional con `skip_serializing_if`. Clientes existentes lo ignoran. |
| Agregar `bazaar_info` a `RegisterResourceRequest` | **NO** | Campo opcional con `serde(default)`. Requests existentes siguen funcionando. |
| Agregar tipos `BazaarExtension`, `BazaarInfo`, etc. | **NO** | Tipos nuevos, no modifican existentes. |
| Agregar validacion JSON Schema al settlement | **NO** | La validacion solo afecta si el recurso se cataloga con info enriched. El settlement sigue funcionando. |
| Agregar dependencia `jsonschema` | **NO** | Solo afecta build, no API. |

**Conclusion: NINGUN cambio rompe compatibilidad existente.** Todos los cambios son aditivos.

### Riesgo de la API de descubrimiento

La respuesta de `GET /discovery/resources` cambiaria ligeramente:

**Antes:**
```json
{
  "url": "https://api.example.com/data",
  "type": "http",
  "description": "Data API",
  "accepts": [...]
}
```

**Despues (si el recurso tiene bazaar info):**
```json
{
  "url": "https://api.example.com/data",
  "type": "http",
  "description": "Data API",
  "accepts": [...],
  "bazaarInfo": {
    "input": { "type": "http", "method": "GET", "queryParams": {"city": "string"} },
    "output": { "type": "json", "example": {"temp": 72} }
  }
}
```

Esto es **retrocompatible** - clientes que no conocen `bazaarInfo` simplemente lo ignoran.

---

## Dependencias

### Dependencias nuevas requeridas

| Crate | Version sugerida | Proposito | Tamano |
|-------|-----------------|-----------|--------|
| `jsonschema` | `0.28+` | Validacion JSON Schema Draft 2020-12 | ~500KB compilado |

**Alternativa sin dependencia nueva**: Se podria hacer una validacion simplificada (verificar que `info.input.type == "http"` y que `method` esta en la lista valida) sin usar un validador completo de JSON Schema. Esto no cumple estrictamente el spec, pero reduce la superficie de dependencias.

### Dependencias existentes que se reutilizan

| Crate | Uso actual | Uso adicional |
|-------|-----------|---------------|
| `serde_json` | Serializacion general | Parseo de `BazaarExtension` |
| `serde` | Derive macros | Derive para nuevos tipos |

---

## Evaluacion de Riesgos

### Riesgo 1: Complejidad de validacion JSON Schema (MEDIO)

**Problema**: El crate `jsonschema` agrega complejidad al build y puede tener issues de compatibilidad con nuestro Rust edition 2021.

**Mitigacion**: Probar compilacion con `jsonschema` antes de integrar. Alternativa: validacion simplificada sin JSON Schema completo.

### Riesgo 2: Incompatibilidad de formatos entre facilitadores (BAJO)

**Problema**: Los 12 facilitadores externos que agregamos pueden no enviar la extension bazaar en sus responses. El campo `bazaar_info` estaria vacio para la mayoria de recursos agregados.

**Mitigacion**: `bazaar_info` es opcional. Los recursos sin esta informacion siguen siendo validos y utiles.

### Riesgo 3: Aumento del tamano de respuesta (BAJO)

**Problema**: Agregar `bazaar_info` a cada recurso en `GET /discovery/resources` aumenta el payload de respuesta.

**Mitigacion**: Usamos `skip_serializing_if = "Option::is_none"` - solo se serializa si existe. La mayoria de recursos no tendran esta informacion inicialmente.

### Riesgo 4: Performance de validacion JSON Schema (BAJO)

**Problema**: Validar JSON Schema en cada settlement podria impactar latencia.

**Mitigacion**: La validacion solo se hace cuando `discoverable=true` Y la extension bazaar esta presente, lo cual es una fraccion minima de settlements. Ademas, la validacion es rapida (~1ms).

### Riesgo 5: Drift con el spec upstream (MEDIO)

**Problema**: El spec esta marcado como "Downloaded At: 2026-02-03" y podria cambiar.

**Mitigacion**: Nuestra implementacion es un superset compatible. Cambios aditivos del spec no nos afectan. Cambios breaking del spec requeririan una nueva revision.

---

## Estimacion de Esfuerzo

### Desglose por fase

| Fase | Descripcion | Lineas de codigo | Complejidad | Tiempo estimado |
|------|-------------|-----------------|-------------|-----------------|
| 1 | Tipos y estructuras (`BazaarExtension`, `BazaarInfo`, etc.) | ~80 lineas | Baja | 30 min |
| 2 | Validador JSON Schema (`bazaar_validator.rs`) | ~90 lineas | Media | 1 hora |
| 3 | Integracion en settlement handler | ~40 lineas modificadas | Media | 45 min |
| 4 | Integracion en API de registro | ~15 lineas modificadas | Baja | 15 min |
| 5 | Conversion v1 outputSchema | ~30 lineas | Baja | 30 min |
| 6 | Tests unitarios | ~150 lineas | Media | 1 hora |
| 7 | Tests de integracion | ~50 lineas | Media | 30 min |
| **Total** | | **~455 lineas** | **Media** | **~4 horas** |

### Archivos modificados

| Archivo | Tipo de cambio |
|---------|---------------|
| `Cargo.toml` | Agregar dependencia `jsonschema` |
| `src/types_v2.rs` | Agregar tipos `BazaarExtension`, `BazaarInfo`, `BazaarInput`, `BazaarOutput`; modificar `DiscoveryResource` y `RegisterResourceRequest` |
| `src/bazaar_validator.rs` | **NUEVO** - validador de la extension |
| `src/handlers.rs` | Modificar settlement tracking para extraer extension bazaar |
| `src/discovery_aggregator.rs` | Agregar conversion v1 outputSchema (opcional) |
| `src/lib.rs` o `src/main.rs` | Declarar nuevo modulo `bazaar_validator` |

---

## Checklist de Verificacion

### Pre-implementacion

- [ ] Verificar que el crate `jsonschema` compila con Rust edition 2021 y version 1.82+
- [ ] Verificar que la version actual del spec upstream no ha cambiado desde 2026-02-03
- [ ] Revisar si otros facilitadores (Coinbase, PayAI, etc.) ya envian la extension bazaar

### Post-implementacion: Tipos

- [ ] Los nuevos tipos `BazaarExtension`, `BazaarInfo`, `BazaarInput`, `BazaarOutput` compilan
- [ ] `serde_json::from_str` puede parsear el ejemplo GET del spec upstream en `BazaarExtension`
- [ ] `serde_json::from_str` puede parsear el ejemplo POST del spec upstream en `BazaarExtension`
- [ ] `DiscoveryResource` con `bazaar_info: None` serializa igual que antes (retrocompatibilidad)
- [ ] `DiscoveryResource` con `bazaar_info: Some(...)` serializa el campo correctamente
- [ ] `RegisterResourceRequest` sin `bazaar_info` deserializa correctamente (retrocompatibilidad)

### Post-implementacion: Validacion

- [ ] `validate_bazaar_extension()` retorna `NotPresent` cuando no hay extension bazaar
- [ ] `validate_bazaar_extension()` retorna `Valid` para los ejemplos GET y POST del spec
- [ ] `validate_bazaar_extension()` retorna `InvalidSchema` para schema malformado
- [ ] `validate_bazaar_extension()` retorna `ValidationFailed` cuando info no cumple schema
- [ ] La validacion no bloquea settlements - solo enriquece la catalogacion

### Post-implementacion: Integracion

- [ ] `POST /discovery/register` acepta requests con y sin `bazaarInfo`
- [ ] `GET /discovery/resources` incluye `bazaarInfo` solo cuando existe
- [ ] Settlement con `discoverable=true` + extension bazaar cataloga con info enriched
- [ ] Settlement con `discoverable=true` sin extension bazaar sigue funcionando como antes
- [ ] `GET /supported` sigue listando `"bazaar"` en extensions

### Post-implementacion: Compatibilidad

- [ ] Tests existentes pasan sin modificacion
- [ ] Formato de respuesta de `GET /discovery/resources` es backward-compatible
- [ ] El agregador sigue importando recursos de los 12 facilitadores externos
- [ ] El crawler de `/.well-known/x402` sigue funcionando
- [ ] `cargo build --release` compila sin errores
- [ ] `cargo clippy` no reporta warnings nuevos
- [ ] `cargo test` pasa todos los tests

### Verificacion de produccion

```bash
# 1. Verificar que /supported sigue listando bazaar
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.extensions'
# Esperado: ["bazaar"]

# 2. Verificar que /discovery/resources responde correctamente
curl -s https://facilitator.ultravioletadao.xyz/discovery/resources?limit=5 | jq '.items | length'
# Esperado: > 0

# 3. Verificar retrocompatibilidad - registrar recurso sin bazaarInfo
curl -X POST https://facilitator.ultravioletadao.xyz/discovery/register \
  -H "Content-Type: application/json" \
  -d '{"url":"https://test.example.com","type":"http","description":"Test","accepts":[{"scheme":"exact","network":"eip155:8453","asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913","amount":"100000","payTo":"0x1234567890123456789012345678901234567890","maxTimeoutSeconds":300}]}'
# Esperado: 200 OK

# 4. Verificar registro con bazaarInfo
curl -X POST https://facilitator.ultravioletadao.xyz/discovery/register \
  -H "Content-Type: application/json" \
  -d '{"url":"https://test2.example.com","type":"http","description":"Test with Bazaar","accepts":[{"scheme":"exact","network":"eip155:8453","asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913","amount":"100000","payTo":"0x1234567890123456789012345678901234567890","maxTimeoutSeconds":300}],"bazaarInfo":{"input":{"type":"http","method":"GET","queryParams":{"q":"search"}},"output":{"type":"json","example":{"results":[]}}}}'
# Esperado: 200 OK con bazaarInfo en la respuesta

# 5. Verificar que el recurso con bazaarInfo aparece correctamente
curl -s https://facilitator.ultravioletadao.xyz/discovery/resources?limit=1 | jq '.items[0].bazaarInfo'
# Esperado: null o el objeto bazaarInfo si fue registrado
```

---

## Conclusion

Nuestra implementacion Meta-Bazaar es un **superset significativo** de lo que el spec upstream define. El spec se enfoca en la **mecanica de transporte** (como la extension bazaar viaja dentro del protocolo x402), mientras que nuestra implementacion se enfoca en la **infraestructura de descubrimiento** (como catalogar, agregar y exponer recursos).

Las brechas identificadas son:

1. **Falta de tipos `info`/`output`**: No almacenamos informacion sobre como consumir los endpoints descubiertos. Esto es la brecha mas significativa.
2. **Falta de validacion JSON Schema**: El spec requiere validacion obligatoria antes de catalogar.
3. **Falta de conversion v1**: Menor importancia ya que el spec dice que no es obligatorio.

Las brechas son **aditivas** - cerrarlas no requiere cambios destructivos. Nuestra arquitectura existente (4 fuentes, persistencia S3, agregacion) es ortogonal al spec y constituye valor agregado propio.

**Recomendacion**: Implementar las Fases 1-3 (tipos + validacion + integracion settlement) como prioridad. Las Fases 4-5 son opcionales y pueden posponerse.

**Riesgo general**: BAJO. Todos los cambios son aditivos y retrocompatibles.
