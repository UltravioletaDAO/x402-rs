# 03 - Solana Memo Program para Unicidad de Transacciones

**Commit upstream**: `4a1575e40d1d436a5edb7073289a504add8a4460`
**Fecha del commit**: 10 de febrero de 2026
**Autor**: Sergey Ukustov
**Referencia**: https://github.com/coinbase/x402/issues/828
**Prioridad**: ALTA - Previene ataques de transacciones duplicadas

---

## Resumen de la Funcionalidad

### Que hace

El SPL Memo Program (`MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr`) es un programa on-chain de Solana que permite adjuntar datos arbitrarios (texto UTF-8) a una transaccion. Upstream lo utiliza para agregar un **nonce aleatorio de 16 bytes** (codificado en base64) a cada transaccion de pago, garantizando que cada transaccion sea criptograficamente unica.

### Por que importa

Sin el memo aleatorio, dos transacciones de pago identicas (mismo monto, mismo destino, mismo token, mismo blockhash reciente) producen el **mismo hash de transaccion**. Esto abre dos vectores de ataque:

1. **Ataque de replay/duplicacion**: Un atacante podria intentar re-enviar una transaccion ya liquidada si el blockhash aun es valido (~2 minutos en Solana).

2. **Colision de transacciones**: Si dos pagos legitimos coinciden en todos los parametros dentro de la misma ventana de blockhash, Solana rechazaria la segunda como "ya procesada" (AlreadyProcessed), causando que un pago valido falle silenciosamente.

El nonce aleatorio de 16 bytes (128 bits de entropia) asegura que la probabilidad de colision es astronomicamente baja (~1 en 3.4 x 10^38), haciendo cada transaccion unica incluso si todos los demas parametros son identicos.

### Cambios de upstream en resumen

El commit hace tres cosas:

1. **Define `MEMO_PROGRAM_PUBKEY`** como constante estatica en `types.rs`
2. **Agrega `build_random_memo_ix()`** en `client.rs` para construir la instruccion memo
3. **Incluye `MEMO_PROGRAM_PUBKEY`** en la lista de programas permitidos del facilitator (allowlist)
4. **Agrega dependencia `rand = "0.10"`** al crate de Solana

---

## Estado Actual (Nuestra Implementacion)

### Archivo: `/mnt/z/ultravioleta/dao/x402-rs/src/chain/solana.rs`

Actualmente **NO tenemos soporte para el Memo Program**. Nuestra implementacion:

- **No define `MEMO_PROGRAM_PUBKEY`** - No existe la constante en nuestro codigo
- **No genera memos aleatorios** - Las transacciones no incluyen instrucciones memo
- **No permite instrucciones memo** - Nuestro verificador flexible (`find_transfer_instruction`) no valida explicitamente programas permitidos por ID, pero tampoco rechaza instrucciones desconocidas activamente; simplemente busca la instruccion de transferencia por tipo de programa (spl_token/spl_token_2022)
- **Ya tenemos `rand = "0.8"`** en Cargo.toml (usado por NEAR/ed25519-dalek), pero no lo usamos en solana.rs

### Diferencia arquitectonica clave

Upstream tiene una **estructura de workspace modular** con crates separados:
- `x402-chain-solana/src/v1_solana_exact/client.rs` - Lado del cliente (construye transacciones)
- `x402-chain-solana/src/v1_solana_exact/facilitator.rs` - Lado del facilitador (verifica y liquida)
- `x402-chain-solana/src/v1_solana_exact/types.rs` - Tipos compartidos

Nuestro fork tiene una **estructura monolitica**:
- `src/chain/solana.rs` - Todo en un solo archivo (~1159 lineas)

En upstream, la separacion client/facilitator significa:
- El **client** genera el memo (funcion `build_random_memo_ix()`)
- El **facilitator** solo necesita aceptar el programa memo en su allowlist

En nuestro fork, somos **solo facilitador** (no somos client). Esto es importante porque:

> **El facilitador NO genera memos. Solo necesita ACEPTAR transacciones que contengan memos.**

Los clientes (wallets, SDKs) son los que construyen las transacciones con memos. Nosotros solo verificamos y liquidamos.

---

## Analisis Detallado de la Implementacion Upstream

### 1. Constante `MEMO_PROGRAM_PUBKEY` (types.rs)

```rust
// Archivo upstream: crates/chains/x402-chain-solana/src/v1_solana_exact/types.rs

use solana_pubkey::{Pubkey, pubkey};

/// SPL Memo program ID - used to add transaction uniqueness and prevent duplicate transaction attacks
/// See: https://github.com/coinbase/x402/issues/828
pub static MEMO_PROGRAM_PUBKEY: Pubkey = pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
```

**Nota**: Upstream cambio de `std::sync::LazyLock` + `.parse()` a `pubkey!()` macro directa. Este es un cambio de estilo que tambien aplico al renombrar `PHANTOM_LIGHTHOUSE_PROGRAM` a `PHANTOM_LIGHTHOUSE_PROGRAM_PUBKEY`.

### 2. Funcion `build_random_memo_ix()` (client.rs - SOLO LADO CLIENTE)

```rust
// Archivo upstream: crates/chains/x402-chain-solana/src/v1_solana_exact/client.rs

use x402_types::util::Base64Bytes;

/// Build a memo instruction with a random nonce for transaction uniqueness.
/// This prevents duplicate transaction attacks by ensuring each transaction has a unique message.
/// The SPL Memo program requires valid UTF-8 data, so we hex-encode the random bytes.
fn build_random_memo_ix() -> Instruction {
    // Generate 16 random bytes for transaction uniqueness
    let nonce: [u8; 16] = rand::random();
    let memo_data = Base64Bytes::encode(nonce).to_string();

    Instruction::new_with_bytes(
        MEMO_PROGRAM_PUBKEY,
        memo_data.as_bytes(),
        Vec::new(), // Empty accounts - SPL Memo doesn't require signers
    )
}
```

**Detalles tecnicos:**
- Genera 16 bytes aleatorios con `rand::random()` (usa `rand 0.10`)
- Los codifica en base64 para cumplir con el requisito de UTF-8 del Memo Program
- La instruccion tiene `Vec::new()` como cuentas (el Memo Program no requiere firmantes)
- El resultado es ~24 bytes de datos memo (16 bytes en base64 = 24 caracteres)

### 3. Integracion en la construccion de transacciones (client.rs)

```rust
// Dentro de build_signed_transfer_transaction():

// Build memo instruction for transaction uniqueness (prevents duplicate transaction attacks)
let memo_ix = build_random_memo_ix();
let full_transfer_instructions = vec![transfer_instruction, memo_ix];
let (msg_to_sim, instructions) =
    build_message_to_simulate(*fee_payer, &full_transfer_instructions, fee, recent_blockhash)?;

let estimated_cu = estimate_compute_units(rpc_client, &msg_to_sim).await?;

let cu_ix = ComputeBudgetInstruction::set_compute_unit_limit(estimated_cu);
let msg = {
    let mut final_instructions = Vec::with_capacity(instructions.len() + 2);
    final_instructions.push(cu_ix);
    final_instructions.extend(instructions);
    // final_instructions ahora contiene: [ComputeLimit, ComputePrice, Transfer, Memo]
    MessageV0::try_compile(fee_payer, &final_instructions, &[], recent_blockhash)
        .map_err(|e| X402Error::SigningError(format!("{e:?}")))?
};
```

**Estructura final de instrucciones en la transaccion:**
| Indice | Instruccion | Programa |
|--------|-------------|----------|
| 0 | SetComputeUnitLimit | ComputeBudget |
| 1 | SetComputeUnitPrice | ComputeBudget |
| 2 | TransferChecked | spl_token / spl_token_2022 |
| 3 | Memo (nonce aleatorio) | SPL Memo Program |

### 4. Allowlist del facilitador (facilitator.rs)

```rust
// Archivo upstream: crates/chains/x402-chain-solana/src/v1_solana_exact/facilitator.rs

fn default_allowed_program_ids() -> Vec<Address> {
    vec![
        Address::new(PHANTOM_LIGHTHOUSE_PROGRAM_PUBKEY),
        Address::new(MEMO_PROGRAM_PUBKEY),
    ]
}
```

El facilitador upstream tiene un sistema de allowlist/blocklist configurable. Las instrucciones adicionales (indice 3+) deben usar programas que esten en la allowlist. Antes solo estaba Phantom Lighthouse; ahora tambien el Memo Program.

### 5. V2 hereda automaticamente

El facilitador V2 (`v2_solana_exact/facilitator.rs`) reutiliza la config de V1:

```rust
pub type V2SolanaExactFacilitatorConfig = V1SolanaExactFacilitatorConfig;
```

Y el cliente V2 reutiliza `build_signed_transfer_transaction` de V1, asi que el memo se incluye automaticamente.

---

## Plan de Implementacion

Dado que nuestro fork es **solo facilitador** (no somos client SDK), los cambios son minimos. Solo necesitamos:

1. Definir la constante `MEMO_PROGRAM_PUBKEY`
2. Aceptar transacciones que contengan instrucciones memo

### Paso 1: Definir `MEMO_PROGRAM_PUBKEY` en `solana.rs`

**Archivo**: `/mnt/z/ultravioleta/dao/x402-rs/src/chain/solana.rs`

**Ubicacion**: Linea 26, junto a `ATA_PROGRAM_PUBKEY`

**Cambio exacto** - agregar despues de la linea 26:

```rust
// ANTES (linea 26):
const ATA_PROGRAM_PUBKEY: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

// DESPUES (lineas 26-30):
const ATA_PROGRAM_PUBKEY: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

/// SPL Memo program ID - instrucciones memo se usan para unicidad de transacciones.
/// Previene ataques de transacciones duplicadas al agregar un nonce aleatorio.
/// Ver: https://github.com/coinbase/x402/issues/828
const MEMO_PROGRAM_PUBKEY: Pubkey = pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
```

**Nota**: Usamos `const` (no `pub static`) porque:
- Es consistente con `ATA_PROGRAM_PUBKEY` en nuestro archivo
- La macro `pubkey!()` de `solana_sdk` evalua en tiempo de compilacion
- No necesita ser publica (solo se usa dentro de `solana.rs`)

### Paso 2: Aceptar instrucciones memo en la verificacion

Nuestra verificacion usa un enfoque **flexible por escaneo** (diferente a upstream que usa posiciones fijas). Esto significa que nuestro `find_transfer_instruction()` ya ignora instrucciones que no son spl_token/spl_token_2022. Sin embargo, hay un punto critico: la **verificacion del fee payer**.

#### Analisis del flujo actual de verificacion:

Nuestra funcion `verify_transfer()` (linea 675) hace:

1. `verify_compute_budget_instructions()` - Busca instrucciones de compute budget
2. `find_transfer_instruction()` - Busca la instruccion TransferChecked
3. **Fee payer safety check** (lineas 754-771) - Verifica que el fee payer NO aparezca en NINGUNA cuenta de instruccion

El punto 3 es el critico. Revisemos que pasa con una instruccion memo:

```rust
// Lineas 755-771 de solana.rs:
let fee_payer_pubkey = self.keypair.pubkey();
for instruction in transaction.message.instructions().iter() {
    for account_idx in instruction.accounts.iter() {
        let account = transaction
            .message
            .static_account_keys()
            .get(*account_idx as usize)
            .ok_or(FacilitatorLocalError::DecodingError(
                "invalid_account_index".to_string(),
            ))?;

        if *account == fee_payer_pubkey {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_fee_payer_included_in_instruction_accounts".to_string(),
            ));
        }
    }
}
```

**Veredicto**: La instruccion memo se construye con `Vec::new()` (cero cuentas):

```rust
Instruction::new_with_bytes(
    MEMO_PROGRAM_PUBKEY,
    memo_data.as_bytes(),
    Vec::new(), // <-- SIN CUENTAS
)
```

Cuando esta instruccion se compila en un `CompiledInstruction`, el campo `accounts` sera un vector vacio. Por lo tanto, el bucle interior `for account_idx in instruction.accounts.iter()` **no ejecutara ninguna iteracion** para la instruccion memo. Esto significa que:

> **La instruccion memo YA PASA la verificacion de fee payer sin cambios.**

#### Analisis del flujo de verificacion completo:

| Paso | Que pasa con instruccion memo? | Resultado |
|------|-------------------------------|-----------|
| `verify_compute_budget_instructions()` | Ignora instrucciones que no son ComputeBudget | OK - memo ignorada |
| `find_transfer_instruction()` | Solo busca spl_token/spl_token_2022 | OK - memo ignorada |
| Fee payer check | Itera cuentas de cada instruccion; memo tiene 0 cuentas | OK - no hay conflicto |
| Simulacion RPC | La simulacion ejecuta todas las instrucciones | OK - memo es valida on-chain |

**Conclusion**: Nuestra implementacion actual ya acepta transacciones con instrucciones memo sin rechazarlas. Sin embargo, debemos **agregar validacion explicita** para ser mas robustos.

### Paso 3: (Recomendado) Agregar validacion explicita de programas conocidos

Aunque nuestra implementacion ya funciona, es buena practica validar explicitamente que las instrucciones adicionales usen programas conocidos. Esto protege contra inyeccion de instrucciones maliciosas.

**Archivo**: `/mnt/z/ultravioleta/dao/x402-rs/src/chain/solana.rs`

**Agregar lista de programas permitidos** (despues de `MEMO_PROGRAM_PUBKEY`, ~linea 31):

```rust
/// Phantom Lighthouse program ID - programa de seguridad inyectado por Phantom wallet en mainnet.
/// Ver: https://github.com/coinbase/x402/issues/828
const PHANTOM_LIGHTHOUSE_PROGRAM_PUBKEY: Pubkey = pubkey!("L2TExMFKdjpN9kozasaurPirfHy9P8sbXoAN1qA3S95");

/// Lista de programas permitidos en instrucciones adicionales.
/// Solo estos programas pueden aparecer fuera de ComputeBudget y spl_token.
const ALLOWED_ADDITIONAL_PROGRAMS: &[Pubkey] = &[
    MEMO_PROGRAM_PUBKEY,
    PHANTOM_LIGHTHOUSE_PROGRAM_PUBKEY,
];
```

**Agregar funcion de validacion** (como metodo de `SolanaProvider`, despues de `verify_compute_budget_instructions`, ~linea 673):

```rust
/// Valida que las instrucciones adicionales (ni ComputeBudget, ni spl_token, ni spl_token_2022)
/// usen programas de la lista de permitidos.
fn validate_additional_programs(
    &self,
    transaction: &VersionedTransaction,
) -> Result<(), FacilitatorLocalError> {
    let instructions = transaction.message.instructions();
    let static_keys = transaction.message.static_account_keys();
    let compute_budget_id = solana_sdk::compute_budget::ID;

    for (idx, instruction) in instructions.iter().enumerate() {
        let program_id = instruction.program_id(static_keys);

        // Saltar programas conocidos del protocolo base
        if *program_id == compute_budget_id
            || *program_id == spl_token::ID
            || *program_id == spl_token_2022::ID
            || *program_id == ATA_PROGRAM_PUBKEY
        {
            continue;
        }

        // Verificar que el programa esta en la lista de permitidos
        if !ALLOWED_ADDITIONAL_PROGRAMS.contains(program_id) {
            tracing::warn!(
                instruction_index = idx,
                program_id = %program_id,
                "Rejected transaction: unknown program in additional instruction"
            );
            return Err(FacilitatorLocalError::DecodingError(
                format!("unknown_program_in_instruction_{}: {}", idx, program_id),
            ));
        }

        tracing::debug!(
            instruction_index = idx,
            program_id = %program_id,
            "Accepted additional instruction from allowed program"
        );
    }

    Ok(())
}
```

### Paso 4: Integrar la validacion en `verify_transfer()`

**Archivo**: `/mnt/z/ultravioleta/dao/x402-rs/src/chain/solana.rs`

**Ubicacion**: Dentro de `verify_transfer()`, entre el paso 2 (find_transfer_instruction) y el paso 3 (fee payer check), ~linea 750.

```rust
// ANTES (linea 746-750):
// 2. Find and verify the transfer instruction (can be at any position)
let (_transfer_idx, transfer_instruction) = self
    .find_transfer_instruction(&transaction, requirements)
    .await?;

// 3. Fee payer safety check

// DESPUES:
// 2. Find and verify the transfer instruction (can be at any position)
let (_transfer_idx, transfer_instruction) = self
    .find_transfer_instruction(&transaction, requirements)
    .await?;

// 2.5. Validate additional programs (memo, Phantom Lighthouse, etc.)
self.validate_additional_programs(&transaction)?;

// 3. Fee payer safety check
```

### Paso 5: (Opcional) Agregar limite maximo de instrucciones

Upstream tiene un limite configurable de 10 instrucciones maximo. Esto previene abuse via transacciones con cientos de instrucciones inutil.

**Agregar al inicio de `verify_transfer()`**, despues de deserializar la transaccion (~linea 737):

```rust
// Limite maximo de instrucciones por transaccion
const MAX_INSTRUCTION_COUNT: usize = 10;

let instruction_count = transaction.message.instructions().len();
if instruction_count > MAX_INSTRUCTION_COUNT {
    tracing::warn!(
        instruction_count = instruction_count,
        max = MAX_INSTRUCTION_COUNT,
        "Rejected transaction: too many instructions"
    );
    return Err(FacilitatorLocalError::DecodingError(
        format!("too_many_instructions: {} > {}", instruction_count, MAX_INSTRUCTION_COUNT),
    ));
}
```

---

## Dependencias

### Nuevas dependencias requeridas: NINGUNA

Nuestro Cargo.toml ya tiene todo lo necesario:

| Dependencia | Version actual | Uso |
|-------------|---------------|-----|
| `solana-sdk` | 2.3.1 | Macro `pubkey!()`, tipos de Solana |
| `rand` | 0.8 | Ya disponible (usado por NEAR) - **pero NO lo necesitamos** porque no generamos memos |
| `spl-token` | 8.0.0 | Ya disponible |
| `spl-token-2022` | 9.0.0 | Ya disponible |

**Nota sobre `rand` version**: Upstream actualizo a `rand = "0.10"` para la funcion `rand::random()` que genera el nonce. Sin embargo, como nosotros **no generamos memos** (somos solo facilitador), no necesitamos esta dependencia para esta funcionalidad. La version 0.8 que ya tenemos permanece sin cambios.

**Nota sobre `spl-memo` crate**: Upstream NO usa el crate `spl-memo` como dependencia directa. Solo define la Pubkey del programa como constante. El crate `spl-memo 6.0.0` ya existe en nuestro `Cargo.lock` como dependencia transitiva de `spl-token-2022`, pero no necesitamos agregarlo explicitamente.

---

## Evaluacion de Riesgos

### Riesgo 1: Transacciones existentes sin memo - BAJO

**Escenario**: Clientes antiguos que no agregan instrucciones memo.

**Impacto**: Nuestros cambios son **aditivos** (solo agregamos validacion de programas). Transacciones sin memo (3 instrucciones: ComputeLimit + ComputePrice + Transfer) siguen funcionando exactamente igual porque `validate_additional_programs()` solo valida instrucciones que NO son ComputeBudget o spl_token.

**Mitigacion**: Ninguna necesaria - backward compatible.

### Riesgo 2: Programas desconocidos rechazados - MEDIO

**Escenario**: Un wallet nuevo agrega instrucciones de un programa no incluido en nuestra allowlist.

**Impacto**: La transaccion seria rechazada con error descriptivo.

**Mitigacion**: Monitorear logs en CloudWatch para `"unknown_program_in_instruction"`. Si aparece un programa legitimo, agregarlo a `ALLOWED_ADDITIONAL_PROGRAMS`. Mantener la lista sincronizada con upstream.

### Riesgo 3: Incremento en compute units - BAJO

**Escenario**: La instruccion memo consume compute units adicionales.

**Impacto**: El SPL Memo Program consume ~300-500 compute units por instruccion (insignificante vs. el limite de 200K-400K). Ademas, el compute unit limit se estima via simulacion antes de la transaccion, asi que se ajusta automaticamente.

**Mitigacion**: Ninguna necesaria.

### Riesgo 4: Datos memo maliciosos - MUY BAJO

**Escenario**: Un atacante incluye datos memo extremadamente grandes.

**Impacto**: El Memo Program de Solana acepta hasta ~566 bytes de datos por instruccion (limite del runtime). Esto no afecta la seguridad del pago.

**Mitigacion**: La simulacion de transaccion ya valida esto. Opcionalmente, podriamos agregar validacion de tamano de datos memo, pero no es necesario.

### Riesgo 5: Divergencia con upstream - BAJO

**Escenario**: Upstream agrega mas programas a la allowlist.

**Impacto**: Transacciones de clientes upstream podrian ser rechazadas si usan programas nuevos no en nuestra lista.

**Mitigacion**: Revisar la allowlist en cada sync trimestral con upstream. La constante `ALLOWED_ADDITIONAL_PROGRAMS` esta centralizada para facil mantenimiento.

---

## Estimacion de Esfuerzo

### Lineas de codigo

| Cambio | Lineas nuevas | Lineas modificadas |
|--------|--------------|-------------------|
| Constantes (`MEMO_PROGRAM_PUBKEY`, `PHANTOM_LIGHTHOUSE_PROGRAM_PUBKEY`, `ALLOWED_ADDITIONAL_PROGRAMS`) | ~12 | 0 |
| Funcion `validate_additional_programs()` | ~30 | 0 |
| Integracion en `verify_transfer()` | ~3 | 0 |
| Limite maximo de instrucciones (opcional) | ~10 | 0 |
| **Total** | **~55** | **0** |

### Complejidad

| Aspecto | Evaluacion |
|---------|-----------|
| Complejidad de codigo | Baja - Constantes y un bucle de validacion |
| Riesgo de regresion | Muy bajo - Cambios aditivos, backward compatible |
| Cambios en dependencias | Ninguno |
| Cambios en API publica | Ninguno |
| Cambios en configuracion | Ninguno |
| Cambios en Terraform/infra | Ninguno |

### Tiempo estimado

- Implementacion: **15-20 minutos**
- Testing en devnet: **30-45 minutos**
- Total: **~1 hora**

---

## Checklist de Verificacion

### Pre-implementacion

- [ ] Leer y entender este documento completo
- [ ] Verificar que `solana_sdk::pubkey::pubkey!` macro acepta la direccion del Memo Program
- [ ] Confirmar que no hay conflictos con la version actual de `src/chain/solana.rs`

### Implementacion

- [ ] Agregar `MEMO_PROGRAM_PUBKEY` (linea ~27 de solana.rs)
- [ ] Agregar `PHANTOM_LIGHTHOUSE_PROGRAM_PUBKEY` (linea ~31 de solana.rs)
- [ ] Agregar `ALLOWED_ADDITIONAL_PROGRAMS` (linea ~34 de solana.rs)
- [ ] Agregar funcion `validate_additional_programs()` (metodo de `SolanaProvider`)
- [ ] Integrar `validate_additional_programs()` en `verify_transfer()` (~linea 750)
- [ ] (Opcional) Agregar `MAX_INSTRUCTION_COUNT` y validacion en `verify_transfer()`
- [ ] Ejecutar `cargo build --release` - verificar compilacion sin errores
- [ ] Ejecutar `just clippy-all` - verificar sin warnings

### Testing en Devnet

#### Test 1: Transaccion normal sin memo (backward compatibility)

```bash
# Iniciar facilitador localmente
cargo run --release

# Verificar endpoint de salud
curl -s http://localhost:8080/health | jq

# Verificar soporte de Solana devnet
curl -s http://localhost:8080/supported | jq '.kinds[] | select(.network == "solana-devnet")'

# Ejecutar test de pago Solana existente (sin memo)
cd tests/integration
python test_usdc_payment.py --network solana-devnet
```

**Resultado esperado**: El pago funciona exactamente como antes. Ninguna regresion.

#### Test 2: Transaccion con instruccion memo (nuevo)

```python
# Script de test: tests/integration/test_solana_memo.py
# Este test construye una transaccion con instruccion memo y la envia al facilitador

import json
import base64
import requests
from solders.keypair import Keypair
from solders.pubkey import Pubkey
from solders.instruction import Instruction, AccountMeta
from solders.message import MessageV0
from solders.transaction import VersionedTransaction
from solders.compute_budget import set_compute_unit_limit, set_compute_unit_price
import struct
import os

MEMO_PROGRAM_ID = Pubkey.from_string("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr")

def build_memo_instruction():
    """Construir instruccion memo con nonce aleatorio"""
    nonce = os.urandom(16)
    memo_data = base64.b64encode(nonce)
    return Instruction(
        program_id=MEMO_PROGRAM_ID,
        accounts=[],  # Sin cuentas
        data=memo_data,
    )

# ... (construir transaccion completa con Transfer + Memo)
# ... (enviar a POST /verify y POST /settle)
```

**Resultado esperado**: El facilitador acepta la transaccion con memo y la liquida correctamente.

#### Test 3: Transaccion con programa desconocido (si se implementa validate_additional_programs)

```bash
# Enviar transaccion con programa no permitido - debe ser rechazada
# (Requiere construir una transaccion custom con un programa arbitrario)
```

**Resultado esperado**: Error `"unknown_program_in_instruction"`.

#### Test 4: Verificar en Solana Explorer

```bash
# Despues de una liquidacion exitosa con memo, verificar en explorer:
# https://explorer.solana.com/tx/<TX_SIG>?cluster=devnet
# Debe mostrar la instruccion memo con datos base64
```

### Post-implementacion

- [ ] Verificar que no hay warnings en `cargo build --release 2>&1`
- [ ] Verificar logs: no deben aparecer errores relacionados con memo
- [ ] Confirmar backward compatibility: clientes sin memo siguen funcionando
- [ ] Documentar el cambio en `docs/CHANGELOG.md`
- [ ] Actualizar version si es necesario

---

## Codigo Completo de Referencia

Para facilitar la implementacion, aqui esta el diff completo que se debe aplicar a `/mnt/z/ultravioleta/dao/x402-rs/src/chain/solana.rs`:

```diff
--- a/src/chain/solana.rs
+++ b/src/chain/solana.rs
@@ -24,6 +24,17 @@ use crate::types::{Scheme, X402Version};

 const ATA_PROGRAM_PUBKEY: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

+/// SPL Memo program ID - instrucciones memo se usan para unicidad de transacciones.
+/// Previene ataques de transacciones duplicadas al agregar un nonce aleatorio.
+/// Ver: https://github.com/coinbase/x402/issues/828
+const MEMO_PROGRAM_PUBKEY: Pubkey = pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
+
+/// Phantom Lighthouse program ID - programa de seguridad inyectado por Phantom wallet en mainnet.
+/// Ver: https://github.com/coinbase/x402/issues/828
+const PHANTOM_LIGHTHOUSE_PROGRAM_PUBKEY: Pubkey = pubkey!("L2TExMFKdjpN9kozasaurPirfHy9P8sbXoAN1qA3S95");
+
+/// Limite maximo de instrucciones por transaccion para prevenir abuse.
+const MAX_INSTRUCTION_COUNT: usize = 10;
+
 #[derive(Clone, Debug)]
 pub struct SolanaChain {
     pub network: Network,
@@ -673,6 +684,52 @@ impl SolanaProvider {
         Ok(())
     }

+    /// Valida que el numero de instrucciones no exceda el maximo permitido.
+    fn validate_instruction_count(
+        &self,
+        transaction: &VersionedTransaction,
+    ) -> Result<(), FacilitatorLocalError> {
+        let count = transaction.message.instructions().len();
+        if count > MAX_INSTRUCTION_COUNT {
+            tracing::warn!(
+                instruction_count = count,
+                max = MAX_INSTRUCTION_COUNT,
+                "Rejected transaction: too many instructions"
+            );
+            return Err(FacilitatorLocalError::DecodingError(
+                format!("too_many_instructions: {} > {}", count, MAX_INSTRUCTION_COUNT),
+            ));
+        }
+        Ok(())
+    }
+
+    /// Valida que las instrucciones adicionales usen programas permitidos.
+    /// Instrucciones de ComputeBudget, spl_token, spl_token_2022, y ATA se permiten
+    /// implicitamente. Cualquier otro programa debe estar en la lista de permitidos
+    /// (MEMO_PROGRAM_PUBKEY, PHANTOM_LIGHTHOUSE_PROGRAM_PUBKEY).
+    fn validate_additional_programs(
+        &self,
+        transaction: &VersionedTransaction,
+    ) -> Result<(), FacilitatorLocalError> {
+        let instructions = transaction.message.instructions();
+        let static_keys = transaction.message.static_account_keys();
+        let compute_budget_id = solana_sdk::compute_budget::ID;
+
+        for (idx, instruction) in instructions.iter().enumerate() {
+            let program_id = instruction.program_id(static_keys);
+
+            // Saltar programas conocidos del protocolo base
+            if *program_id == compute_budget_id
+                || *program_id == spl_token::ID
+                || *program_id == spl_token_2022::ID
+                || *program_id == ATA_PROGRAM_PUBKEY
+                || *program_id == MEMO_PROGRAM_PUBKEY
+                || *program_id == PHANTOM_LIGHTHOUSE_PROGRAM_PUBKEY
+            {
+                continue;
+            }
+
+            tracing::warn!(
+                instruction_index = idx,
+                program_id = %program_id,
+                "Rejected transaction: unknown program in additional instruction"
+            );
+            return Err(FacilitatorLocalError::DecodingError(
+                format!("unknown_program_in_instruction_{}: {}", idx, program_id),
+            ));
+        }
+
+        Ok(())
+    }
+
     async fn verify_transfer(
         &self,
         request: &VerifyRequest,
@@ -739,6 +796,12 @@ impl SolanaProvider {
             "Decoded user-signed transaction"
         );

+        // 0. Validar numero de instrucciones
+        self.validate_instruction_count(&transaction)?;
+
         // Flexible verification: find instructions by program ID, not fixed positions
         // This allows Phantom to add extra instructions while we still validate the critical ones

@@ -750,6 +813,9 @@ impl SolanaProvider {
             .find_transfer_instruction(&transaction, requirements)
             .await?;

+        // 2.5. Validate additional programs (memo, Phantom Lighthouse, etc.)
+        self.validate_additional_programs(&transaction)?;
+
         // 3. Fee payer safety check
```

---

## Resumen Ejecutivo

| Aspecto | Valor |
|---------|-------|
| **Que** | Soporte para SPL Memo Program en verificacion de transacciones Solana |
| **Por que** | Prevenir ataques de transacciones duplicadas; compatibilidad con clientes upstream |
| **Impacto** | Solo lado facilitador (verificacion); no afecta generacion de transacciones |
| **Archivos modificados** | 1 (`src/chain/solana.rs`) |
| **Lineas nuevas** | ~55 |
| **Dependencias nuevas** | 0 |
| **Backward compatible** | Si - transacciones sin memo siguen funcionando |
| **Riesgo** | Bajo |
| **Tiempo estimado** | ~1 hora (implementacion + testing) |
