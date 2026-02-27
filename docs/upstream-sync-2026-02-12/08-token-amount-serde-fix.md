# 08 - TokenAmount Serde Fix: Serializacion Decimal vs Hex

**Commit upstream**: `f7383ec` — `chore(x402-chain-eip155): replace U256 with TokenAmount for value field: Serialize as decimals`
**Autor**: Sergey Ukustov
**Fecha**: 5 febrero 2026
**Prioridad**: CRITICA (potencial bug de produccion en serializacion de pagos)

---

## Descripcion del Bug

### El Problema Raiz

En el upstream, el campo `value` del struct `ExactEvmPayloadAuthorization` estaba tipado como `U256` directamente (de `alloy_primitives`). El tipo `U256` de Alloy tiene un comportamiento de serializacion problematico:

- **`U256` serializa como hex** (con prefijo `0x`) cuando usa `#[derive(Serialize)]` por defecto de Alloy
- **El protocolo x402 espera strings decimales** (e.g., `"1000000"` para 1 USDC)

Esto significa que un pago de 1 USDC ($1.00 = 1,000,000 unidades base) se serializaria como:
- **Incorrecto (hex)**: `"0xf4240"` o `"0x00000000000000000000000000000000000000000000000000000000000f4240"`
- **Correcto (decimal)**: `"1000000"`

### Impacto del Bug

Cuando `U256` se serializa como hex en el campo `value`:

1. **Verificacion fallida**: Los clientes JavaScript del protocolo x402 reciben el valor como hex y lo interpretan incorrectamente, o directamente fallan al parsear
2. **Montos incorrectos**: Si el cliente intenta interpretar `"0xf4240"` como decimal, obtiene un numero completamente diferente
3. **Pagos rechazados**: La comparacion entre `value` del payload y `maxAmountRequired` de los requirements falla por formato incompatible
4. **Interoperabilidad rota**: Cualquier cliente x402 (TypeScript SDK, Go SDK) que espera strings decimales no puede procesar respuestas con hex

### La Solucion Upstream

El commit `f7383ec` reemplaza `U256` con `TokenAmount` (un newtype wrapper sobre `U256`) que implementa serializacion/deserializacion personalizada:

```rust
// ANTES (buggy):
pub struct ExactEvmPayloadAuthorization {
    pub value: U256,  // Serializa como hex: "0xf4240"
}

// DESPUES (fix):
pub struct ExactEvmPayloadAuthorization {
    pub value: TokenAmount,  // Serializa como decimal: "1000000"
}
```

Donde `TokenAmount` implementa:
```rust
impl Serialize for TokenAmount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())  // U256::to_string() produce decimal
    }
}

impl<'de> Deserialize<'de> for TokenAmount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}
```

---

## Evaluacion de Impacto en Nuestra Produccion

### Estado Actual: YA ESTAMOS PROTEGIDOS

**Nuestro fork NO tiene este bug.** Ya implementamos `TokenAmount` con serializacion decimal correcta en una version anterior. Veamos la evidencia:

#### 1. Nuestro `TokenAmount` en `src/types.rs` (linea 622-910)

```rust
// src/types.rs linea 622-626
#[derive(Debug, Copy, Clone, PartialEq, Ord, PartialOrd, Eq, Hash)]
pub struct TokenAmount(pub U256);
```

Con serializacion correcta (lineas 898-910):
```rust
impl<'de> Deserialize<'de> for TokenAmount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let string = String::deserialize(deserializer)?;
        let value = U256::from_str(&string).map_err(serde::de::Error::custom)?;
        Ok(TokenAmount(value))
    }
}

impl Serialize for TokenAmount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())  // Decimal!
    }
}
```

#### 2. Nuestro `ExactEvmPayloadAuthorization` ya usa `TokenAmount`

```rust
// src/types.rs linea 456-465
pub struct ExactEvmPayloadAuthorization {
    pub from: EvmAddress,
    pub to: EvmAddress,
    pub value: TokenAmount,      // <-- YA CORRECTO
    pub valid_after: UnixTimestamp,
    pub valid_before: UnixTimestamp,
    pub nonce: HexEncodedNonce,
}
```

#### 3. `ExactEvmPayment` en `src/chain/evm.rs` tambien usa `TokenAmount`

```rust
// src/chain/evm.rs linea 180-198
pub struct ExactEvmPayment {
    pub chain: EvmChain,
    pub from: EvmAddress,
    pub to: EvmAddress,
    pub value: TokenAmount,      // <-- YA CORRECTO
    pub valid_after: UnixTimestamp,
    pub valid_before: UnixTimestamp,
    pub nonce: HexEncodedNonce,
    pub signature: EvmSignature,
}
```

#### 4. Conversiones `TokenAmount -> U256` se hacen correctamente en settlement

En `src/chain/evm.rs`, las funciones `transferWithAuthorization_0` y `transferWithAuthorization_1` convierten de `TokenAmount` a `U256` al construir la llamada on-chain:

```rust
// src/chain/evm.rs linea 1469
let value: U256 = payment.value.into();  // TokenAmount -> U256 para llamada EVM
```

Esto es identico a lo que hace el upstream despues del fix.

---

## Ubicaciones Exactas del Codigo

### Archivos Relevantes

| Archivo | Lineas | Contenido |
|---------|--------|-----------|
| `src/types.rs` | 622-928 | Definicion completa de `TokenAmount` con serde decimal |
| `src/types.rs` | 454-465 | `ExactEvmPayloadAuthorization` con `value: TokenAmount` |
| `src/types.rs` | 1267-1284 | `PaymentRequirements` con `max_amount_required: TokenAmount` |
| `src/types_v2.rs` | 82-104 | `PaymentRequirementsV2` con `amount: TokenAmount` |
| `src/chain/evm.rs` | 180-198 | `ExactEvmPayment` con `value: TokenAmount` |
| `src/chain/evm.rs` | 1087-1105 | `TransferWithAuthorization0Call` con `value: U256` (interno, no serializado) |
| `src/chain/evm.rs` | 1534-1550 | `TransferWithAuthorization1Call` con `value: U256` (interno, no serializado) |
| `src/chain/evm.rs` | 1189-1207 | `assert_enough_value()` comparando `U256` (correcto) |
| `src/chain/evm.rs` | 1434-1444 | `assert_valid_payment()` convierte `TokenAmount -> U256` |

### Flujo de Datos del Campo `value`

```
Cliente (JSON) --> Deserializacion --> TokenAmount (decimal string)
    |
    v
ExactEvmPayloadAuthorization.value: TokenAmount
    |
    v
ExactEvmPayment.value: TokenAmount (copia interna)
    |
    v
transferWithAuthorization_0() / _1():
    let value: U256 = payment.value.into();  // Conversion a U256 para Alloy
    |
    v
Llamada on-chain ERC-3009: value como uint256
```

---

## Analisis del Fix Upstream

### Archivos Modificados en `f7383ec`

| Archivo Upstream | Cambio |
|-----------------|--------|
| `v1_eip155_exact/types.rs` | `value: U256` -> `value: TokenAmount` |
| `v1_eip155_exact/client.rs` | `params.amount` -> `params.amount.into()` en 2 sitios |
| `v1_eip155_exact/facilitator.rs` | `authorization.value` -> `authorization.value.into()` en 2 sitios |
| `v2_eip155_exact/facilitator.rs` | Mismo patron de conversion `.into()` en 2 sitios |

### Patron del Fix

En todos los casos, el fix agrega `.into()` donde `TokenAmount` se pasa a funciones que esperan `U256`:

```rust
// ANTES:
assert_enough_value(&authorization.value, &amount_required)?;

// DESPUES:
assert_enough_value(&authorization.value.into(), &amount_required)?;
```

Esto es necesario porque `assert_enough_value` recibe `&U256`, y `TokenAmount` necesita `.into()` para convertirse.

### Comparacion con Nuestro Codigo

En nuestro `src/chain/evm.rs`, ya hacemos esta conversion explicitamente:

```rust
// src/chain/evm.rs linea 1437-1438
let value: U256 = payment_payload.authorization.value.into();
assert_enough_value(&payer, &value, &amount_required)?;
```

La conversion es identica en semantica, solo difiere en estilo (binding explicito vs `.into()` inline).

---

## Plan de Fix

### Accion Requerida: NINGUNA (Codigo ya correcto)

Nuestro fork ya implementa `TokenAmount` con serializacion decimal correcta en todas las ubicaciones criticas. No se requiere ningun cambio.

### Verificacion Detallada

| Punto de Verificacion | Estado | Evidencia |
|-----------------------|--------|-----------|
| `ExactEvmPayloadAuthorization.value` usa `TokenAmount` | OK | `src/types.rs:461` |
| `ExactEvmPayment.value` usa `TokenAmount` | OK | `src/chain/evm.rs:189` |
| `TokenAmount::Serialize` produce decimal | OK | `src/types.rs:906-910` |
| `TokenAmount::Deserialize` acepta decimal | OK | `src/types.rs:898-904` |
| `PaymentRequirements.max_amount_required` usa `TokenAmount` | OK | `src/types.rs:1274` |
| `PaymentRequirementsV2.amount` usa `TokenAmount` | OK | `src/types_v2.rs:93` |
| Conversiones `TokenAmount -> U256` en settlement | OK | `src/chain/evm.rs:1437,1469,1574` |
| x402r conversion `value` a `TokenAmount` | OK | `src/types_v2.rs:516,617` |

---

## Compatibilidad Hacia Atras

### Formatos Soportados por Nuestro `TokenAmount::Deserialize`

Nuestro deserializador usa `U256::from_str()` de Alloy, que acepta:

1. **Strings decimales**: `"1000000"` -> OK (formato principal del protocolo)
2. **Strings hex con prefijo**: `"0xf4240"` -> OK (retrocompatible con clientes viejos)
3. **Enteros numericos**: NO soportado (serializa como string, no numero)

Esto significa que nuestro facilitador puede recibir payloads de:
- Clientes x402 modernos (envian decimal strings) -> OK
- Clientes con el bug de hex (si existieran) -> OK tambien, por retrocompatibilidad de `U256::from_str`

### Formatos de Salida

Nuestro serializador SIEMPRE produce strings decimales:
```json
{
  "value": "1000000"
}
```

Esto es correcto y compatible con todos los SDKs x402 (TypeScript, Go, Python).

### Diferencia Critica con Upstream

El upstream usa `U256::from_str_radix(s, 10)` en su `TokenAmount::from_str()`, que SOLO acepta decimal:

```rust
// Upstream TokenAmount
fn from_str(s: &str) -> Result<Self, Self::Err> {
    let u256 = U256::from_str_radix(s, 10)
        .map_err(|_| "invalid token amount".to_string())?;
    Ok(Self(u256))
}
```

Nuestro fork usa `U256::from_str()` que es mas permisivo (acepta tanto hex como decimal). Esto es un **beneficio** en terminos de retrocompatibilidad, pero podria ser un riesgo si alguien envara accidentalmente un valor hex que se interpreta como decimal.

**Riesgo bajo**: En la practica, los valores de token son siempre menores que `2^64`, y nunca serian ambiguos entre hex y decimal (no contienen letras a-f a menos que tengan prefijo `0x`).

---

## Testing

### Tests Existentes

No tenemos tests unitarios especificos para la serializacion de `TokenAmount`. El test mas cercano es el de `PaymentRequirementsV2` en `types_v2.rs` que deserializa `"amount": "1000000"`:

```rust
// src/types_v2.rs linea 1328-1341
fn test_payment_requirements_v2_serde() {
    let json = r#"{"amount": "1000000", ...}"#;
    let reqs: PaymentRequirementsV2 = serde_json::from_str(json).unwrap();
}
```

### Tests Recomendados (Baja Prioridad)

Si se quisiera agregar cobertura explicita, los tests serian:

```rust
#[test]
fn test_token_amount_serializes_as_decimal_string() {
    let amount = TokenAmount::from(1_000_000u64);
    let json = serde_json::to_string(&amount).unwrap();
    assert_eq!(json, "\"1000000\"");
    // No debe producir hex
    assert!(!json.contains("0x"));
}

#[test]
fn test_token_amount_deserializes_decimal_string() {
    let amount: TokenAmount = serde_json::from_str("\"1000000\"").unwrap();
    assert_eq!(amount.0, U256::from(1_000_000u64));
}

#[test]
fn test_token_amount_roundtrip() {
    let original = TokenAmount::from(999_999_999u64);
    let json = serde_json::to_string(&original).unwrap();
    let parsed: TokenAmount = serde_json::from_str(&json).unwrap();
    assert_eq!(original, parsed);
}

#[test]
fn test_exact_evm_payload_authorization_value_serializes_decimal() {
    let auth = ExactEvmPayloadAuthorization {
        from: "0x1234567890123456789012345678901234567890".parse().unwrap(),
        to: "0x1234567890123456789012345678901234567890".parse().unwrap(),
        value: TokenAmount::from(5_000_000u64),  // 5 USDC
        valid_after: UnixTimestamp(0),
        valid_before: UnixTimestamp(2000000000),
        nonce: HexEncodedNonce([0u8; 32]),
    };
    let json = serde_json::to_string(&auth).unwrap();
    assert!(json.contains("\"value\":\"5000000\""));
}
```

**Prioridad**: Baja. El codigo ya funciona correctamente en produccion. Estos tests serian de regresion.

---

## Dependencias

- **Ninguna dependencia nueva** necesaria
- `TokenAmount` ya esta definido y funcional en `src/types.rs`
- No se requiere actualizar Cargo.toml

---

## Evaluacion de Riesgo

| Riesgo | Probabilidad | Impacto | Mitigacion |
|--------|-------------|---------|------------|
| Bug de serializacion hex en produccion | **NULA** | Critico | Ya usamos `TokenAmount` con decimal serde |
| Regresion al hacer merge de upstream | **Baja** | Alto | Nuestro `TokenAmount` esta en `src/types.rs` local, no en un crate separado |
| Incompatibilidad de `from_str` (hex vs decimal only) | **Minima** | Bajo | Nuestro `from_str` es mas permisivo (acepta ambos), lo cual es un beneficio |
| Conflicto de merge en `ExactEvmPayloadAuthorization` | **Baja** | Bajo | Ambos lados ya usan `TokenAmount`; merge trivial |

### Riesgo Global: BAJO

No se requiere accion alguna. Nuestro codigo ya tiene el fix equivalente implementado.

---

## Estimacion de Esfuerzo

| Tarea | Esfuerzo |
|-------|----------|
| Verificar que no hay regresion | 10 min (ya completado en este analisis) |
| Agregar tests unitarios (opcional) | 30 min |
| Cambios de codigo | **0 min** (no requeridos) |

**Total**: 0 horas de trabajo requerido. El codigo ya esta correcto.

---

## Checklist de Verificacion

- [x] `ExactEvmPayloadAuthorization.value` es `TokenAmount` (no `U256`)
- [x] `ExactEvmPayment.value` es `TokenAmount` (no `U256`)
- [x] `TokenAmount::Serialize` produce strings decimales
- [x] `TokenAmount::Deserialize` acepta strings decimales
- [x] `PaymentRequirements.max_amount_required` es `TokenAmount`
- [x] `PaymentRequirementsV2.amount` es `TokenAmount`
- [x] Conversiones `TokenAmount -> U256` en `transferWithAuthorization_0/1` son correctas
- [x] `assert_enough_value()` recibe `U256` despues de conversion desde `TokenAmount`
- [x] x402r format conversion (`VerifyRequestX402r.to_v1()`) construye `TokenAmount` correctamente
- [x] x402r nested format (`VerifyRequestX402rNested.to_v1()`) construye `TokenAmount` correctamente
- [x] No hay ningun lugar donde `U256` se serialice directamente en un campo `value` de payload
- [ ] (Opcional) Agregar tests unitarios de serializacion/deserializacion de `TokenAmount`
- [ ] (Opcional) Agregar test de roundtrip JSON para `ExactEvmPayloadAuthorization`

---

## Resumen Ejecutivo

**El bug que upstream corrigio en `f7383ec` NO nos afecta.** Nuestro fork ya implemento `TokenAmount` como newtype wrapper sobre `U256` con serializacion/deserializacion decimal personalizada en `src/types.rs`. Todos los campos `value` en los structs de autorizacion EVM (`ExactEvmPayloadAuthorization`, `ExactEvmPayment`) ya usan `TokenAmount` en lugar de `U256` crudo.

Las conversiones de `TokenAmount` a `U256` para llamadas on-chain (ERC-3009 `transferWithAuthorization`) se realizan explicitamente con `.into()` en los puntos correctos de `src/chain/evm.rs`.

**No se requiere ningun cambio de codigo.** El unico trabajo pendiente (opcional) seria agregar tests unitarios explicitos para documentar y proteger este comportamiento de serializacion.
