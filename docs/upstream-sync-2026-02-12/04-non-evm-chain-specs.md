# 04 - Validacion de Implementaciones Non-EVM contra Especificaciones Formales

**Fecha**: 2026-02-12
**Tipo**: Auditoria de cumplimiento (Compliance Audit)
**Alcance**: Stellar, Sui, Algorand, NEAR
**Upstream**: `x402-rs/x402-rs` v1.1.3 (specs descargadas 2026-02-03)
**Fork**: `UltravioletaDAO/x402-rs` v1.32.1

---

## Tabla de Contenidos

1. [Resumen Ejecutivo](#resumen-ejecutivo)
2. [Stellar - Auditoria Completa](#stellar---auditoria-completa)
3. [Sui - Auditoria Completa](#sui---auditoria-completa)
4. [Algorand - Auditoria Completa](#algorand---auditoria-completa)
5. [NEAR - Documentacion sin Spec](#near---documentacion-sin-spec)
6. [Matriz de Riesgo Consolidada](#matriz-de-riesgo-consolidada)
7. [Plan de Remediacion Priorizado](#plan-de-remediacion-priorizado)
8. [Estimaciones de Esfuerzo](#estimaciones-de-esfuerzo)
9. [Listas de Verificacion](#listas-de-verificacion)

---

## Resumen Ejecutivo

Upstream ha publicado especificaciones formales para tres cadenas non-EVM: **Stellar**, **Sui** y **Algorand**. Nuestra implementacion precede a estas especificaciones, lo que crea divergencias significativas en payload format, flujo de verificacion y controles de seguridad.

### Hallazgos Criticos

| Cadena | Gaps Criticos | Gaps Medios | Gaps Bajos | Cumplimiento Estimado |
|--------|--------------|-------------|------------|----------------------|
| Stellar | 4 | 3 | 2 | ~55% |
| Sui | 3 | 2 | 2 | ~50% |
| Algorand | 2 | 3 | 1 | ~70% |
| NEAR | N/A (sin spec) | N/A | N/A | N/A |

**Riesgo principal**: Incompatibilidad de formato de payload. Clientes construidos contra las specs de upstream NO podran interactuar con nuestro facilitator sin cambios.

---

## Stellar - Auditoria Completa

### Resumen de la Especificacion

**Archivo upstream**: `docs/specs/schemes/exact/scheme_exact_stellar.md`
**Version del protocolo**: Solo v2 (NO v1)

La spec de Stellar define un flujo donde:
1. El cliente construye una **transaccion completa** con `invokeHostFunction` llamando a `transfer(from, to, amount)` en el contrato token SEP-41.
2. El cliente firma las **auth entries** (no la transaccion completa).
3. El cliente envia la **transaccion completa codificada en XDR base64** al facilitator.
4. El facilitator **reconstruye** la transaccion con su propia cuenta como source, preservando operaciones y auth entries.
5. El facilitator simula, firma y envia la transaccion reconstruida.

**Diferencia fundamental con nuestra implementacion**: La spec espera una transaccion completa en el payload. Nosotros recibimos solo la **authorization entry** (no la transaccion).

### Nuestra Implementacion

**Archivo**: `/mnt/z/ultravioleta/dao/x402-rs/src/chain/stellar.rs` (1586 lineas)
**Archivo de tipos**: `/mnt/z/ultravioleta/dao/x402-rs/src/types.rs` (lineas 491-508)

**Funciones clave**:
- `verify_payment()` (linea 858): Verificacion principal
- `verify_authorization_signature()` (linea 560): Verificacion de firma
- `verify_multisig_authorization()` (linea 650): Multi-sig
- `build_unsigned_transaction()` (linea 955): Construccion de transaccion
- `build_signed_envelope()` (linea 1032): Envolvente firmado
- `submit_transaction()` (linea 1152): Envio a la red
- `compute_auth_entry_preimage()` (linea 775): Calculo de preimagen para firma

**Flujo actual**:
1. Recibimos `ExactStellarPayload` con campos: `from`, `to`, `amount`, `token_contract`, `authorization_entry_xdr`, `nonce`, `signature_expiration_ledger`.
2. Decodificamos la auth entry individual.
3. Verificamos la firma de la auth entry.
4. Construimos la transaccion completa internamente.
5. Simulamos, firmamos y enviamos.

### Matriz de Cumplimiento Stellar

#### 1. Validacion de Protocolo

| Requisito Spec | Estado | Detalle |
|---|---|---|
| `x402Version` DEBE ser `2` | **NO CUMPLE** | Nuestra `supported()` retorna `X402Version::V1` (linea 1539). No verificamos version en verify/settle. |
| `payload.accepted.scheme` y `requirements.scheme` DEBEN ser `"exact"` | **CUMPLE** | Verificamos en linea 889-895 (`payload.scheme != requirements.scheme`). |
| `payload.accepted.network` DEBE coincidir con `requirements.network` | **CUMPLE** | Verificamos en lineas 872-886. |

#### 2. Estructura de Transaccion

| Requisito Spec | Estado | Detalle |
|---|---|---|
| La transaccion DEBE contener exactamente 1 operacion `invokeHostFunction` | **NO APLICA** | No recibimos transaccion completa; la construimos nosotros. |
| El tipo de funcion DEBE ser `hostFunctionTypeInvokeContract` | **CUMPLE PARCIAL** | Verificamos en `build_unsigned_transaction()` linea 974: extraemos `ContractFn`, rechazamos `CreateContractHostFn`. |
| La direccion del contrato DEBE coincidir con `requirements.asset` | **NO CUMPLE** | No comparamos `token_contract` del payload contra `requirements.asset`. Solo usamos el token_contract proporcionado por el cliente. |
| El nombre de funcion DEBE ser `"transfer"` con exactamente 3 argumentos | **NO CUMPLE** | No validamos el nombre de la funcion ni el numero de argumentos en la auth entry. |
| Argumento 1 (to) DEBE ser igual a `requirements.payTo` exactamente | **NO CUMPLE** | No comparamos el destino de la auth entry contra `requirements.payTo`. |
| Argumento 2 (amount) DEBE ser igual a `requirements.amount` exactamente (como i128) | **NO CUMPLE** | No comparamos el monto de la auth entry contra `requirements.amount`. |

#### 3. Auth Entries

| Requisito Spec | Estado | Detalle |
|---|---|---|
| DEBE contener auth entries firmadas para la direccion `from` | **CUMPLE** | Verificamos firma en `verify_authorization_signature()`. |
| Auth entries DEBEN usar credential type `sorobanCredentialsAddress` solamente | **CUMPLE PARCIAL** | Aceptamos `SourceAccount` (linea 568) sin error; la spec dice solo `sorobanCredentialsAddress`. |
| `rootInvocation` NO DEBE contener `subInvocations` que autoricen operaciones adicionales | **NO CUMPLE** | No verificamos que `sub_invocations` este vacio. |
| El facilitator DEBE verificar que todos los firmantes requeridos firmaron | **CUMPLE** | Verificamos firma de la direccion esperada. |
| La expiracion de auth entry NO DEBE exceder `currentLedger + ceil(maxTimeoutSeconds / estimatedLedgerSeconds)` | **CUMPLE PARCIAL** | Verificamos que no haya expirado (linea 912), pero no que no exceda el maximo permitido por `maxTimeoutSeconds`. |

#### 4. Seguridad del Facilitator (CRITICO)

| Requisito Spec | Estado | Detalle |
|---|---|---|
| Source account de la transaccion del cliente NO DEBE ser el facilitator | **NO APLICA** | No recibimos transaccion del cliente. |
| Source account de la operacion NO DEBE ser el facilitator | **NO APLICA** | Construimos la operacion nosotros. |
| El facilitator NO DEBE ser la direccion `from` en el transfer | **NO CUMPLE** | No verificamos que `stellar_payload.from != self.public_key`. |
| La direccion del facilitator NO DEBE aparecer en auth entries | **NO CUMPLE** | No inspeccionamos las auth entries buscando nuestra direccion. |
| La simulacion DEBE emitir eventos mostrando SOLO los cambios de balance esperados | **NO CUMPLE** | No verificamos eventos de la simulacion. Solo verificamos que no haya error. |

#### 5. Simulacion

| Requisito Spec | Estado | Detalle |
|---|---|---|
| DEBE re-simular contra estado actual del ledger | **CUMPLE** | Simulamos en `submit_transaction()` linea 1210. |
| La simulacion DEBE tener exito sin errores | **CUMPLE** | Verificamos error en linea 1224. |
| DEBE emitir eventos confirmando el cambio de balance exacto | **NO CUMPLE** | No verificamos eventos de la simulacion. |

#### 6. Settlement

| Requisito Spec | Estado | Detalle |
|---|---|---|
| Reconstruir transaccion con cuenta del facilitator como source | **CUMPLE** | Linea 1012-1015, usamos facilitator como source. |
| Preservar operaciones y auth entries del cliente | **CUMPLE** | Linea 992, copiamos auth entry del cliente. |
| Firmar con clave del facilitator | **CUMPLE** | Linea 1058-1059. |
| Enviar via `sendTransaction` | **CUMPLE** | Linea 1322. |
| Poll para confirmacion | **CUMPLE** | `wait_for_transaction()` linea 1361. |
| SettleResponse.transaction: hash hex 64 chars | **CUMPLE** | Retornamos `TransactionHash::Stellar([u8; 32])`. |
| SettleResponse.payer: direccion del cliente (no facilitator) | **CUMPLE** | Linea 1522, usamos `verification.payer`. |

#### 7. Formato de Payload

| Requisito Spec | Estado | Detalle |
|---|---|---|
| Payload.transaction: base64 XDR de transaccion Stellar completa | **NO CUMPLE** | Nuestro payload tiene campos individuales (from, to, amount, etc.), no una transaccion completa. |
| x402Version: 2 | **NO CUMPLE** | Reportamos v1. |
| Extra.areFeesSponsored | **NO CUMPLE** | No incluimos este campo en /supported ni lo verificamos. |

### Gaps Encontrados - Stellar

#### GAP-S1: Formato de Payload Incompatible (CRITICO)

**Spec**: El payload debe contener `{ "transaction": "<base64 XDR>" }` - una transaccion Stellar completa.
**Nosotros**: Nuestro `ExactStellarPayload` tiene campos desglosados: `from`, `to`, `amount`, `token_contract`, `authorization_entry_xdr`, `nonce`, `signature_expiration_ledger`.

**Impacto**: Clientes que implementen la spec de upstream NO podran comunicarse con nuestro facilitator. Incompatibilidad total de formato.

**Remediacion**:
1. Agregar soporte para el formato de transaccion completa como una nueva variante del payload.
2. Mantener el formato actual como legacy para clientes existentes.
3. Detectar automaticamente el formato basado en la presencia del campo `transaction` vs `authorization_entry_xdr`.
4. Archivos a modificar:
   - `src/types.rs`: Agregar nuevo struct `ExactStellarPayloadV2` con campo `transaction: String`.
   - `src/chain/stellar.rs`: Nuevo metodo `verify_payment_v2()` que parsea la transaccion completa.
   - `src/chain/stellar.rs`: Extraer auth entries, operaciones y argumentos de la transaccion decodificada.

#### GAP-S2: Falta de Validacion de Estructura de Transaccion (CRITICO)

**Spec**: La spec requiere verificar exactamente 1 operacion `invokeHostFunction`, funcion `transfer`, 3 argumentos, y que `to` y `amount` coincidan con requirements.

**Nosotros**: No validamos la estructura interna de la auth entry contra los requirements. Confiamos en los campos del payload (`to`, `amount`) sin compararlos con lo que realmente esta firmado en la auth entry.

**Impacto**: Un atacante podria enviar una auth entry que transfiera a una direccion diferente de `payTo`, pero con campos `to`/`amount` del payload que pasen la verificacion.

**Remediacion**:
1. Decodificar los argumentos de `root_invocation.function` (ContractFn).
2. Verificar que `function_name == "transfer"` y `args.len() == 3`.
3. Verificar que `args[1]` (Address to) coincida con `requirements.payTo`.
4. Verificar que `args[2]` (i128 amount) coincida con `requirements.amount`.
5. Verificar que el contrato en `contract_address` coincida con `requirements.asset`.
6. Archivo: `src/chain/stellar.rs`, metodo `verify_payment()` linea 858.

#### GAP-S3: Falta Verificacion de Seguridad del Facilitator (CRITICO)

**Spec**: Multiples verificaciones para que el facilitator no sea enganyado en transferir sus propios fondos.

**Nosotros**: No verificamos que `from != facilitator_address`. No verificamos que el facilitator no aparezca en auth entries.

**Impacto**: Potencial ataque donde un payload malicioso hace que el facilitator transfiera sus propios fondos.

**Remediacion**:
1. Agregar check: `if stellar_payload.from == self.public_key { return Err(...) }`.
2. Inspeccionar auth entry credentials para verificar que no contengan la direccion del facilitator.
3. Despues de simulacion, verificar que los eventos emitidos solo muestren cambios de balance esperados.
4. Archivo: `src/chain/stellar.rs`, metodos `verify_payment()` y `submit_transaction()`.

#### GAP-S4: Falta Verificacion de Eventos de Simulacion (CRITICO)

**Spec**: "La simulacion DEBE emitir eventos mostrando SOLO los cambios de balance esperados, y NINGUN OTRO CAMBIO DE BALANCE."

**Nosotros**: Solo verificamos que la simulacion no tenga errores. No inspeccionamos los eventos.

**Impacto**: Una transaccion podria tener side-effects no deseados (transferencias adicionales ocultas) que pasen la simulacion pero drenen fondos.

**Remediacion**:
1. Parsear los eventos de la respuesta de simulacion.
2. Verificar que solo existan eventos de transfer con los parametros esperados.
3. Rechazar si hay eventos de balance inesperados.
4. Esto requiere decodificar los diagnostic events de Soroban.

#### GAP-S5: Version x402 Incorrecta (MEDIO)

**Spec**: Solo v2, `x402Version` DEBE ser `2`.
**Nosotros**: `supported()` retorna `X402Version::V1` (linea 1539).

**Remediacion**: Cambiar a `X402Version::V2` en `supported()`. Agregar validacion de version en `verify_payment()`.

#### GAP-S6: No Verificacion de Sub-Invocaciones (MEDIO)

**Spec**: `rootInvocation` NO DEBE contener `subInvocations`.
**Nosotros**: No verificamos `root_invocation.sub_invocations`.

**Remediacion**: Agregar check `if !auth_entry.root_invocation.sub_invocations.is_empty() { return Err(...) }`.

#### GAP-S7: Aceptamos SourceAccount Credentials (MEDIO)

**Spec**: Solo `sorobanCredentialsAddress`.
**Nosotros**: Aceptamos `SourceAccount` sin error (linea 568).

**Remediacion**: Retornar error si `credentials == SorobanCredentials::SourceAccount`.

#### GAP-S8: No Verificacion de Maximo Timeout (BAJO)

**Spec**: La expiracion no debe exceder `currentLedger + ceil(maxTimeoutSeconds / estimatedLedgerSeconds)`.
**Nosotros**: Solo verificamos que no haya expirado, no que sea excesivamente lejana.

**Remediacion**: Calcular el maximo permitido usando `maxTimeoutSeconds` y verificar que `signature_expiration_ledger <= max_allowed`.

#### GAP-S9: Extra.areFeesSponsored No Expuesto (BAJO)

**Spec**: El campo `extra.areFeesSponsored` debe estar en PaymentRequirements.
**Nosotros**: No lo incluimos en `/supported`.

**Remediacion**: Agregar campo en `SupportedPaymentKindExtra`.

---

## Sui - Auditoria Completa

### Resumen de la Especificacion

**Archivo upstream**: `docs/specs/schemes/exact/scheme_exact_sui.md`
**Version del protocolo**: v2

La spec de Sui define un flujo donde:
1. El cliente construye una transaccion completa para transferir un `Coin<T>` al resource server.
2. El cliente firma la transaccion.
3. El payload contiene: `{ "signature": "<base64>", "transaction": "<base64 BCS>" }`.
4. El facilitator verifica la firma sobre la transaccion.
5. El facilitator simula la transaccion.
6. El facilitator verifica que los outputs de simulacion muestren el balance correcto.
7. Si hay sponsorship, el facilitator co-firma y envia.

**Diferencia con nuestra implementacion**: La spec usa campos `signature` y `transaction` en el payload. Nosotros usamos campos `sender_signature`, `transaction_bytes`, `from`, `to`, `amount`, `coin_object_id`.

### Nuestra Implementacion

**Archivo**: `/mnt/z/ultravioleta/dao/x402-rs/src/chain/sui.rs` (610 lineas)
**Archivo de tipos**: `/mnt/z/ultravioleta/dao/x402-rs/src/types.rs` (lineas 550-568)

**Funciones clave**:
- `verify_transaction()` (linea 207): Verificacion de transaccion
- `verify_signature()` (linea 156): Verificacion de firma
- `check_balance()` (linea 289): Verificacion de balance
- `submit_sponsored_transaction()` (linea 333): Envio de transaccion sponsoreada
- `decode_transaction_bytes()` (linea 114): Decodificacion de transaccion

### Matriz de Cumplimiento Sui

#### 1. Verificacion de Red

| Requisito Spec | Estado | Detalle |
|---|---|---|
| Verificar la red es la acordada | **CUMPLE** | Linea 216-222, verificamos network match. |

#### 2. Verificacion de Firma

| Requisito Spec | Estado | Detalle |
|---|---|---|
| Verificar firma sobre la transaccion proporcionada | **CUMPLE PARCIAL** | Verificamos que el signer de la firma coincida con el sender esperado (linea 175-196), pero no hacemos verificacion criptografica completa de la firma contra los datos de transaccion. Confiamos en que Sui la verifique al enviar. |

#### 3. Simulacion

| Requisito Spec | Estado | Detalle |
|---|---|---|
| Simular la transaccion para asegurar que tendria exito | **NO CUMPLE** | No simulamos. Enviamos directamente via `execute_transaction_block`. |
| Verificar que no haya sido ejecutada/committed ya | **NO CUMPLE** | No hacemos esta verificacion pre-envio. |

#### 4. Verificacion de Outputs

| Requisito Spec | Estado | Detalle |
|---|---|---|
| Verificar que la direccion del resource server vea un cambio de balance igual a `PaymentRequirements.amount` en el `asset` acordado | **CUMPLE PARCIAL** | Verificamos amount contra requirements (linea 252-256) a nivel de payload, pero no verificamos los outputs reales de simulacion/ejecucion. |

#### 5. Settlement

| Requisito Spec | Estado | Detalle |
|---|---|---|
| Broadcasting de la transaccion con firma del cliente | **CUMPLE** | `submit_sponsored_transaction()` incluye firma del sender y del sponsor. |

#### 6. Sponsorship

| Requisito Spec | Estado | Detalle |
|---|---|---|
| `PaymentRequirements.extra.gasStation` para URL de gas station | **NO CUMPLE** | No exponemos gas station URL. Nuestro sponsorship es implicit. |
| Flujo interactivo con gas station | **NO CUMPLE** | No implementamos el protocolo de gas station. |

#### 7. Formato de Payload

| Requisito Spec | Estado | Detalle |
|---|---|---|
| `payload.signature`: firma del usuario base64 | **CUMPLE** (nombre diferente: `sender_signature`) | |
| `payload.transaction`: transaccion BCS base64 | **CUMPLE** (nombre diferente: `transaction_bytes`) | |
| Payload solo tiene `signature` y `transaction` | **NO CUMPLE** | Nuestro payload incluye campos extra: `from`, `to`, `amount`, `coin_object_id`. |

### Gaps Encontrados - Sui

#### GAP-U1: Formato de Payload Incompatible (CRITICO)

**Spec**: `{ "signature": "...", "transaction": "..." }` - solo 2 campos.
**Nosotros**: `{ "transaction_bytes": "...", "sender_signature": "...", "from": "...", "to": "...", "amount": "...", "coin_object_id": "..." }` - 6 campos con nombres diferentes.

**Impacto**: Incompatibilidad total con clientes que implementen la spec.

**Remediacion**:
1. Agregar nuevo struct `ExactSuiPayloadV2` con `{ signature, transaction }`.
2. Extraer `from`, `to`, `amount` directamente de los datos de transaccion BCS decodificados.
3. Mantener payload legacy con deteccion automatica.
4. Archivos: `src/types.rs`, `src/chain/sui.rs`.

#### GAP-U2: No Simulacion Pre-Envio (CRITICO)

**Spec**: "Simular la transaccion para asegurar que tendria exito y no ha sido ejecutada."
**Nosotros**: Enviamos directamente sin simular. `execute_transaction_block` ejecuta la transaccion en la red.

**Impacto**: Podriamos enviar transacciones que fallen (gastando gas), o transacciones que ya fueron ejecutadas.

**Remediacion**:
1. Antes de `execute_transaction_block`, usar `sui_client.read_api().dry_run_transaction_block()`.
2. Verificar que la simulacion tenga exito.
3. Verificar balance changes en los efectos de simulacion.
4. Archivo: `src/chain/sui.rs`, nuevo metodo `simulate_transaction()`.

#### GAP-U3: No Verificacion de Balance Changes Post-Simulacion (CRITICO)

**Spec**: "Verificar los outputs de la simulacion/ejecucion para asegurar que la direccion del resource server vea un cambio de balance igual al amount en el asset acordado."
**Nosotros**: Verificamos amount a nivel de payload pero no verificamos los efectos reales.

**Impacto**: El payload podria decir amount=X pero la transaccion real podria transferir una cantidad diferente.

**Remediacion**:
1. Despues de simulacion, parsear `balanceChanges` de los efectos.
2. Verificar que `payTo` reciba exactamente `amount` del asset correcto.
3. Rechazar si hay balance changes inesperados.

#### GAP-U4: Gas Station Protocol No Implementado (MEDIO)

**Spec**: Protocolo interactivo de gas station via `PaymentRequirements.extra.gasStation`.
**Nosotros**: Sponsorship es implicit (facilitator siempre sponsor).

**Impacto**: Clientes no pueden construir transacciones sponsoreadas segun el protocolo estandar.

**Remediacion**: Implementar endpoint de gas station o al menos no anunciar sponsorship si no seguimos el protocolo.

#### GAP-U5: Verificacion de Firma Incompleta (MEDIO)

**Spec**: "Verificar la firma es valida sobre la transaccion proporcionada."
**Nosotros**: Solo verificamos que la public key extraida de la firma derive a la direccion esperada. No hacemos verificacion criptografica completa de la firma contra el hash de la transaccion.

**Impacto**: Aceptamos firmas que podrian no ser validas sobre los datos de transaccion especificos.

**Remediacion**:
1. Usar `signature.verify_secure(&IntentMessage::new(Intent::sui_transaction(), &tx_data), sender_address)` en lugar de solo extraer la public key.
2. Archivo: `src/chain/sui.rs`, metodo `verify_signature()`.

#### GAP-U6: Version x402 Incorrecta (BAJO)

**Nosotros**: `supported()` retorna `X402Version::V1` (linea 574).
**Remediacion**: Cambiar a `X402Version::V2`.

#### GAP-U7: Nonce/Replay Protection Ausente (BAJO)

**Nosotros**: No tenemos nonce store para Sui. Dependemos de que Sui rechace transacciones duplicadas a nivel de red.

**Impacto**: Bajo, porque Sui tiene proteccion nativa de replay a nivel de protocolo (digest-based), pero no prevenimos reintentos al facilitator antes de envio.

---

## Algorand - Auditoria Completa

### Resumen de la Especificacion

**Archivo upstream**: `docs/specs/schemes/exact/scheme_exact_algo.md`
**Version del protocolo**: v2 (CAIP-2: `algorand:wGHE2Pwdvd7S12BL5FaOP20EGYesN73ktiC1qzkkit8=`)

La spec de Algorand define un flujo con transaction groups atomicos:
1. El `paymentRequirements` PUEDE incluir un `feePayer` en `extra`.
2. `paymentRequirements.asset` DEBE ser un ASA ID (string representando u64).
3. El payload contiene: `{ "paymentIndex": N, "paymentGroup": ["<base64 msgpack>", ...] }`.
4. Verificacion: decodificar grupo, validar la transaccion de pago (amount, receiver, asset), validar fee transaction, simular.
5. Settlement: facilitator firma fee tx y envia el grupo completo.

### Nuestra Implementacion

**Archivo**: `/mnt/z/ultravioleta/dao/x402-rs/src/chain/algorand.rs` (955 lineas, feature-gated `algorand`)
**Archivo de tipos**: `/mnt/z/ultravioleta/dao/x402-rs/src/types.rs` (lineas 523-535)

**Funciones clave**:
- `verify_payment_group()` (linea 432): Verificacion del grupo atomico
- `validate_fee_transaction()` (linea 396): Validacion de seguridad de fee tx
- `submit_group()` (linea 550): Envio del grupo
- `simulate_group()` (linea 664): Simulacion (actualmente deshabilitada)
- `wait_for_confirmation()` (linea 620): Espera de confirmacion

### Matriz de Cumplimiento Algorand

#### 1. Validacion de Grupo

| Requisito Spec | Estado | Detalle |
|---|---|---|
| `paymentGroup` contiene 16 o menos elementos | **NO CUMPLE** | Solo verificamos `len() >= 2` (linea 436), no maximo de 16. |
| Decodificar todas las transacciones del grupo | **CUMPLE** | Decodificamos fee_tx y payment (lineas 450-457). |

#### 2. Validacion de Transaccion de Pago

| Requisito Spec | Estado | Detalle |
|---|---|---|
| `aamt` (asset amount) coincide con `maxAmountRequired` | **NO CUMPLE** | No comparamos `amount` contra `requirements.max_amount_required`. Solo extraemos el amount. |
| `arcv` (asset receiver) coincide con `payTo` | **NO CUMPLE** | No comparamos `receiver` contra `requirements.pay_to`. Solo extraemos el receiver. |
| ASA ID coincide con `requirements.asset` | **CUMPLE** | Verificamos contra `chain.usdc_asa_id` (linea 509), pero hardcodeado a USDC, no contra requirements. |

#### 3. Validacion de Fee Transaction

| Requisito Spec | Estado | Detalle |
|---|---|---|
| `type` (transaction type) es `pay` | **NO CUMPLE** | Aceptamos tanto `Payment` como `AssetTransferTransaction` (lineas 406-426). La spec dice que SOLO puede ser `pay`. |
| Campos `close`, `rekey`, `amt` deben estar omitidos | **CUMPLE PARCIAL** | Verificamos `rekey_to` y `close_remainder_to`/`close_to` (lineas 398-426), pero no verificamos que `amt == 0`. |
| `fee` es un monto razonable | **NO CUMPLE** | No verificamos que el fee sea razonable. |
| Firmar la transaccion | **CUMPLE** | Firmamos en `submit_group()` linea 565-568. |

#### 4. Simulacion

| Requisito Spec | Estado | Detalle |
|---|---|---|
| Simular contra nodo Algorand | **NO CUMPLE** | La simulacion esta **deshabilitada** (linea 585-593). Comentario: "Simulation is temporarily disabled because rmp_serde::to_vec_named produces msgpack with algonaut's field names". |

#### 5. Settlement

| Requisito Spec | Estado | Detalle |
|---|---|---|
| Enviar grupo firmado via `v2/transactions` | **CUMPLE** | `broadcast_signed_transactions` (linea 596-600). |
| Instant finality - confirmar inclusion en bloque | **CUMPLE** | `wait_for_confirmation()` (linea 620). |
| Retornar tx ID de `paymentGroup[paymentIndex]` | **CUMPLE PARCIAL** | Retornamos el tx_id del broadcast, que corresponde a la primera transaccion del grupo. La spec pide el tx_id de `paymentGroup[paymentIndex]`. |

#### 6. Formato de Payload

| Requisito Spec | Estado | Detalle |
|---|---|---|
| `paymentIndex`: indice de la transaccion de pago | **CUMPLE** | Campo `payment_index` en nuestro payload (linea 529). |
| `paymentGroup`: array de transacciones base64 msgpack | **CUMPLE** | Campo `payment_group` (linea 534). |
| CAIP-2 network identifier | **NO CUMPLE** | Usamos nombres propios (`algorand`, `algorand-testnet`), no CAIP-2 (`algorand:wGHE2Pwdvd7S12BL5FaOP20EGYesN73ktiC1qzkkit8=`). |

### Gaps Encontrados - Algorand

#### GAP-A1: Simulacion Deshabilitada (CRITICO)

**Spec**: "Evaluar el payment group contra el endpoint `simulate` del nodo Algorand."
**Nosotros**: La simulacion esta completamente deshabilitada. Linea 585: "Simulation is temporarily disabled".

**Causa raiz**: `rmp_serde::to_vec_named` produce msgpack con field names de algonaut (ej. "transaction") en lugar de los nombres canonicos de Algorand (ej. "txn"). La API de simulate rechaza esto.

**Impacto**: Enviamos transacciones sin verificar que tendrian exito. Podriamos gastar gas en transacciones fallidas o perder visibilidad sobre ataques.

**Remediacion**:
1. Implementar codificacion msgpack canonica para la API de simulate.
2. Alternativa: Usar la API raw HTTP de algod (no algonaut) para encodear las transacciones con canonical msgpack.
3. Alternativa rapida: Hacer DryRun con `algod.dryrun()` como paso intermedio.
4. Archivo: `src/chain/algorand.rs`, metodo `simulate_group()`.

#### GAP-A2: Falta Validacion de Amount y Receiver contra Requirements (CRITICO)

**Spec**: Verificar que `aamt` coincida con `maxAmountRequired` y `arcv` coincida con `payTo`.
**Nosotros**: Extraemos amount y receiver de la transaccion pero nunca los comparamos con los payment requirements.

**Impacto**: Un atacante podria enviar una transaccion que pague a una direccion diferente o un monto diferente del solicitado.

**Remediacion**:
1. En `verify()` y `settle()` del trait `Facilitator`, despues de llamar `verify_payment_group()`, comparar:
   - `verification.amount` vs `requirements.max_amount_required`.
   - `verification.recipient` vs `requirements.pay_to`.
2. Archivo: `src/chain/algorand.rs`, implementaciones de `verify()` y `settle()`.

#### GAP-A3: Fee Transaction Acepta Tipos Incorrectos (MEDIO)

**Spec**: La fee transaction DEBE ser tipo `pay` (payment de ALGO).
**Nosotros**: `validate_fee_transaction()` acepta cualquier tipo de transaccion.

**Impacto**: Un atacante podria incluir una fee transaction de tipo asset transfer u otro que tenga efectos no deseados.

**Remediacion**:
1. Agregar check en `validate_fee_transaction()`:
   ```rust
   if !matches!(&tx.txn_type, TransactionType::Payment(_)) {
       return Err(AlgorandError::ForbiddenFeeField {
           field: "type must be 'pay'".to_string(),
       });
   }
   ```
2. Verificar que `amt == 0` en la fee transaction.

#### GAP-A4: No Limite de 16 Transacciones (MEDIO)

**Spec**: El grupo puede contener maximo 16 transacciones top-level.
**Nosotros**: Solo verificamos `len() >= 2`, sin maximo.

**Impacto**: Podriamos procesar grupos invalidos. Bajo riesgo directo pero viola la spec.

**Remediacion**: Agregar `if payload.payment_group.len() > 16 { return Err(...) }`.

#### GAP-A5: Fee Amount No Verificado (MEDIO)

**Spec**: Verificar que el `fee` sea un monto razonable.
**Nosotros**: No verificamos el fee de la fee transaction.

**Impacto**: Un fee excesivo drenaria el balance de ALGO del facilitator.

**Remediacion**: Establecer un limite maximo de fee (ej. 10,000 microalgos = 0.01 ALGO) y rechazar si se excede.

#### GAP-A6: Transaction ID del Settlement (BAJO)

**Spec**: Retornar el tx ID de `paymentGroup[paymentIndex]`, no de la primera transaccion.
**Nosotros**: Retornamos el tx_id del broadcast (que corresponde a la primera tx del grupo).

**Remediacion**: Calcular el tx_id del `paymentGroup[paymentIndex]` especificamente.

---

## NEAR - Documentacion sin Spec

### Estado Actual

**NO existe especificacion formal upstream para NEAR.** Upstream no incluye un archivo `scheme_exact_near.md` en `docs/specs/schemes/exact/`.

Nuestra implementacion de NEAR es completamente original, basada en NEP-366 (meta-transactions) y NEP-141 (fungible tokens).

### Nuestra Implementacion

**Archivo**: `/mnt/z/ultravioleta/dao/x402-rs/src/chain/near.rs` (733 lineas)
**Archivo de tipos**: `/mnt/z/ultravioleta/dao/x402-rs/src/types.rs` (lineas 483-488)

**Funciones clave**:
- `verify_payment()` (linea 483): Verificacion de pago
- `verify_delegate_action()` (linea 465): Verificacion de firma
- `decode_signed_delegate_action()` (linea 441): Decodificacion
- `submit_meta_transaction()` (linea 565): Envio de meta-transaccion
- `ensure_recipient_registered()` (linea 410): Auto-registro de destinatario
- `extract_usdc_receiver()` (linea 251): Extraccion de receptor USDC

### Inventario de Caracteristicas NEAR

| Caracteristica | Estado | Detalle |
|---|---|---|
| NEP-366 meta-transactions | **Implementado** | SignedDelegateAction como payload |
| Verificacion de firma de delegate action | **Implementado** | `signed_delegate_action.verify()` (linea 470) |
| Verificacion de contrato USDC | **Implementado** | Compara `receiver_id` con `requirements.asset` (linea 542) |
| Verificacion de red | **Implementado** | Lineas 497-511 |
| Verificacion de scheme | **Implementado** | Lineas 513-519 |
| Auto-registro de destinatarios (storage_deposit) | **Implementado** | `ensure_recipient_registered()` - feature unica |
| Envio de transaccion con gas pagado por facilitator | **Implementado** | `submit_meta_transaction()` |
| Replay protection (nonce) | **NO implementado** | No hay nonce store para NEAR |
| Verificacion de amount contra requirements | **NO implementado** | No extraemos/comparamos amount |
| Verificacion de receiver contra requirements.payTo | **NO implementado** | Extraemos receiver pero no comparamos con payTo |
| Simulacion | **NO implementado** | No simulamos la meta-transaccion |
| Proteccion contra facilitator como sender | **NO implementado** | No verificamos que sender != facilitator |
| Proteccion contra acciones maliciosas en delegate | **PARCIAL** | Solo buscamos `ft_transfer`, pero no rechazamos acciones adicionales |

### Lo que Necesitariamos para Crear Nuestra Propia Spec NEAR

Si quisieramos crear una especificacion formal tipo upstream para NEAR, deberia cubrir:

1. **Formato del Payload**:
   - `{ "signedDelegateAction": "<base64 borsh-encoded SignedDelegateAction>" }`
   - Definir exactamente que acciones son permitidas dentro del DelegateAction.

2. **PaymentRequirements**:
   - `network`: CAIP-2 (ej. `near:mainnet`)
   - `asset`: NEAR account ID del contrato NEP-141 (ej. `usdc.fakes.testnet`)
   - `extra.areFeesSponsored`: siempre true (facilitator paga gas + storage)

3. **Reglas de Verificacion**:
   - La SignedDelegateAction DEBE tener firma valida.
   - El `receiver_id` del DelegateAction DEBE coincidir con `requirements.asset` (contrato token).
   - DEBE contener exactamente UNA accion FunctionCall con `method_name == "ft_transfer"`.
   - Los argumentos de `ft_transfer` DEBEN contener `receiver_id == requirements.payTo` y `amount >= requirements.amount`.
   - NO DEBE contener acciones adicionales (prevent batch exploits).
   - El `sender_id` NO DEBE ser el facilitator.
   - El nonce del DelegateAction DEBE ser valido y no reutilizado.
   - `max_block_height` DEBE ser futuro pero no excesivamente lejano.

4. **Seguridad del Facilitator**:
   - El facilitator NO DEBE aparecer como sender en ninguna accion.
   - Solo acciones `ft_transfer` son permitidas (no `ft_transfer_call` que podria ejecutar codigo arbitrario).
   - Limite en gas adjuntado a las acciones internas.

5. **Settlement**:
   - Facilitator verifica registro del destinatario en contrato token (storage_balance_of).
   - Si no registrado, facilitator ejecuta storage_deposit (paga ~0.00125 NEAR).
   - Facilitator envuelve SignedDelegateAction en Transaction con Action::Delegate.
   - Facilitator firma y envia usando broadcast_tx_commit.
   - Retorna tx hash como confirmacion.

6. **SettlementResponse**:
   - `transaction`: hash hex de la transaccion
   - `payer`: account_id del usuario (sender del delegate action)
   - `network`: CAIP-2 identifier

### Gaps de Seguridad en NEAR (Auto-Auditoria)

#### GAP-N1: No Verificacion de Amount (ALTO)

**Nosotros**: No extraemos ni comparamos el amount de `ft_transfer` contra requirements.

**Remediacion**:
1. En `verify_payment()`, despues de decodificar la SignedDelegateAction, parsear los argumentos de `ft_transfer`.
2. Extraer `amount` de los args JSON.
3. Comparar con `requirements.max_amount_required`.

#### GAP-N2: No Verificacion de Receiver contra PayTo (ALTO)

**Nosotros**: Extraemos `receiver_id` de ft_transfer en `extract_usdc_receiver()` pero nunca lo comparamos con `requirements.pay_to`.

**Remediacion**: Agregar comparacion en `verify_payment()`.

#### GAP-N3: No Proteccion contra Acciones Adicionales (ALTO)

**Nosotros**: Buscamos `ft_transfer` en las acciones pero no rechazamos si hay acciones adicionales (ej. un `ft_transfer_call` oculto que ejecute codigo arbitrario, o un `add_key` que de acceso al atacante).

**Remediacion**:
1. Verificar que `delegate_action.actions.len() == 1`.
2. Verificar que la unica accion sea `FunctionCall` con `method_name == "ft_transfer"`.
3. Rechazar explicitamente `ft_transfer_call`, `add_key`, `delete_key`, `deploy_contract`, etc.

#### GAP-N4: No Proteccion de Facilitator como Sender (MEDIO)

**Nosotros**: No verificamos que `sender_id != self.account_id`.

**Remediacion**: Agregar check al inicio de `verify_payment()`.

#### GAP-N5: No Replay Protection (MEDIO)

**Nosotros**: Sin nonce store para NEAR. Dependemos del nonce de la DelegateAction a nivel de protocolo, pero no prevenimos reintentos al facilitator.

**Remediacion**: Implementar nonce store similar al de Stellar/Algorand.

#### GAP-N6: No Simulacion (BAJO)

**Nosotros**: No simulamos la meta-transaccion antes de enviarla.

**Remediacion**: NEAR no tiene un endpoint de simulacion estandar para meta-transacciones, pero podriamos hacer un view call para verificar balance pre-envio.

---

## Matriz de Riesgo Consolidada

### Riesgos Criticos (Produccion en riesgo inmediato)

| ID | Cadena | Gap | Riesgo | Probabilidad | Impacto |
|---|---|---|---|---|---|
| GAP-S3 | Stellar | Facilitator como sender no verificado | Drenaje de fondos del facilitator | Media | Critico |
| GAP-S2 | Stellar | No validacion de estructura tx | Pago a direccion incorrecta | Media | Critico |
| GAP-A1 | Algorand | Simulacion deshabilitada | Transacciones fallidas, gas desperdiciado | Alta | Alto |
| GAP-A2 | Algorand | No validacion amount/receiver | Pago incorrecto | Media | Critico |
| GAP-N3 | NEAR | Acciones adicionales no rechazadas | Explotacion via acciones maliciosas | Baja-Media | Critico |
| GAP-N1 | NEAR | No validacion de amount | Pago insuficiente aceptado | Media | Alto |

### Riesgos Medios (Incompatibilidad o seguridad parcial)

| ID | Cadena | Gap | Riesgo | Probabilidad | Impacto |
|---|---|---|---|---|---|
| GAP-S1 | Stellar | Formato payload incompatible | Clientes upstream no funcionan | Alta | Alto |
| GAP-U1 | Sui | Formato payload incompatible | Clientes upstream no funcionan | Alta | Alto |
| GAP-U2 | Sui | No simulacion | Transacciones fallidas | Media | Medio |
| GAP-U5 | Sui | Verificacion firma incompleta | Firmas invalidas aceptadas | Baja | Medio |
| GAP-A3 | Algorand | Fee tx tipos incorrectos | Fee tx maliciosa | Baja | Medio |
| GAP-N2 | NEAR | No validacion receiver | Pago a direccion incorrecta | Media | Alto |

### Riesgos Bajos (Violaciones de spec menores)

| ID | Cadena | Gap | Riesgo |
|---|---|---|---|
| GAP-S5 | Stellar | Version x402 incorrecta | Incompatibilidad de version |
| GAP-S8 | Stellar | No max timeout check | Auth entries excesivamente lejanas |
| GAP-S9 | Stellar | areFeesSponsored no expuesto | Discovery incompleto |
| GAP-U6 | Sui | Version x402 incorrecta | Incompatibilidad de version |
| GAP-U7 | Sui | No nonce store | Reintentos al facilitator |
| GAP-A6 | Algorand | TX ID incorrecto en settlement | Referencia incorrecta |

---

## Plan de Remediacion Priorizado

### Fase 1: Seguridad Critica (Sprint 1 - 3-5 dias)

**Objetivo**: Cerrar gaps que ponen en riesgo fondos del facilitator.

| # | Tarea | Archivos | Esfuerzo |
|---|---|---|---|
| 1.1 | Stellar: Verificar facilitator no aparece en auth entries ni como `from` | `src/chain/stellar.rs` | 2h |
| 1.2 | Stellar: Validar estructura de invocacion (transfer, 3 args, contract match) | `src/chain/stellar.rs` | 4h |
| 1.3 | Stellar: Validar amount y to de auth entry contra requirements | `src/chain/stellar.rs` | 2h |
| 1.4 | Stellar: Rechazar SourceAccount credentials y sub-invocaciones | `src/chain/stellar.rs` | 1h |
| 1.5 | Algorand: Validar amount y receiver contra requirements | `src/chain/algorand.rs` | 2h |
| 1.6 | Algorand: Restringir fee tx a tipo `pay`, validar amt=0, limite de fee | `src/chain/algorand.rs` | 2h |
| 1.7 | Algorand: Limite maximo de 16 transacciones en grupo | `src/chain/algorand.rs` | 0.5h |
| 1.8 | NEAR: Rechazar acciones adicionales (solo 1 ft_transfer permitido) | `src/chain/near.rs` | 2h |
| 1.9 | NEAR: Validar amount y receiver_id contra requirements | `src/chain/near.rs` | 2h |
| 1.10 | NEAR: Verificar sender_id != facilitator account_id | `src/chain/near.rs` | 0.5h |
| 1.11 | Sui: Verificacion criptografica completa de firma | `src/chain/sui.rs` | 2h |

**Total Fase 1: ~20 horas**

### Fase 2: Simulacion y Verificacion (Sprint 2 - 3-5 dias)

**Objetivo**: Implementar simulacion donde falta.

| # | Tarea | Archivos | Esfuerzo |
|---|---|---|---|
| 2.1 | Stellar: Verificar eventos de simulacion (balance changes) | `src/chain/stellar.rs` | 6h |
| 2.2 | Sui: Implementar dry_run pre-envio | `src/chain/sui.rs` | 4h |
| 2.3 | Sui: Verificar balance changes en efectos de simulacion | `src/chain/sui.rs` | 4h |
| 2.4 | Algorand: Corregir encoding msgpack para simulate API | `src/chain/algorand.rs` | 8h |
| 2.5 | NEAR: Verificacion de balance pre-envio (view call) | `src/chain/near.rs` | 3h |

**Total Fase 2: ~25 horas**

### Fase 3: Compatibilidad de Formato (Sprint 3 - 5-8 dias)

**Objetivo**: Soporte dual de formatos (legacy + spec).

| # | Tarea | Archivos | Esfuerzo |
|---|---|---|---|
| 3.1 | Stellar: Agregar ExactStellarPayloadV2 con transaccion completa | `src/types.rs`, `src/chain/stellar.rs` | 12h |
| 3.2 | Sui: Agregar ExactSuiPayloadV2 con solo signature+transaction | `src/types.rs`, `src/chain/sui.rs` | 8h |
| 3.3 | Algorand: Agregar soporte CAIP-2 network identifiers | `src/caip2.rs`, `src/chain/algorand.rs` | 4h |
| 3.4 | Actualizar version x402 a V2 para todas las cadenas | Multiples archivos | 2h |
| 3.5 | Agregar areFeesSponsored en extra para Stellar | `src/types.rs` | 1h |
| 3.6 | Deteccion automatica de formato v1/v2 en deserializacion | `src/types.rs` | 4h |

**Total Fase 3: ~31 horas**

### Fase 4: Pulido y NEAR Spec (Sprint 4 - 2-3 dias)

| # | Tarea | Archivos | Esfuerzo |
|---|---|---|---|
| 4.1 | NEAR: Implementar nonce store | `src/chain/near.rs`, `src/nonce_store.rs` | 4h |
| 4.2 | Stellar: Verificacion de max timeout | `src/chain/stellar.rs` | 1h |
| 4.3 | Algorand: Retornar tx_id correcto (paymentIndex) | `src/chain/algorand.rs` | 2h |
| 4.4 | Documentar spec interna para NEAR | `docs/specs/scheme_exact_near.md` | 4h |
| 4.5 | Sui: Implementar nonce store | `src/chain/sui.rs`, `src/nonce_store.rs` | 3h |

**Total Fase 4: ~14 horas**

---

## Estimaciones de Esfuerzo

### Por Cadena

| Cadena | Fase 1 (Seguridad) | Fase 2 (Simulacion) | Fase 3 (Formato) | Fase 4 (Pulido) | Total |
|---|---|---|---|---|---|
| Stellar | 9h | 6h | 13h | 1h | **29h** |
| Sui | 2h | 8h | 10h | 3h | **23h** |
| Algorand | 4.5h | 8h | 4h | 2h | **18.5h** |
| NEAR | 4.5h | 3h | 0h | 8h | **15.5h** |
| **Total** | **20h** | **25h** | **31h** | **14h** | **90h** |

### Por Prioridad

| Prioridad | Esfuerzo | Plazo Recomendado |
|---|---|---|
| Critica (seguridad) | 20h | 1 semana |
| Alta (simulacion) | 25h | 2 semanas |
| Media (compatibilidad) | 31h | 3 semanas |
| Baja (pulido) | 14h | 4 semanas |

### Riesgo de Implementacion

- **Stellar Fase 3** tiene el mayor riesgo tecnico: parsear transacciones XDR completas es complejo y requiere manejo extensivo de tipos Stellar.
- **Algorand Fase 2** tiene riesgo medio: la codificacion msgpack canonica requiere investigacion sobre los field names exactos que espera la API de simulate.
- **Sui Fase 2** tiene riesgo bajo: la API de dry_run esta bien documentada en el SDK de Sui.

---

## Listas de Verificacion

### Verificacion de Cumplimiento Stellar

```
PRE-REQUISITOS:
[ ] Facilitator desplegado con Stellar habilitado
[ ] Fondos de test en wallet del facilitator (testnet)
[ ] Acceso a Soroban RPC testnet

VERIFICACION DE SEGURIDAD (Fase 1):
[ ] Test: Enviar payload con from=facilitator_address -> DEBE rechazar
[ ] Test: Enviar auth entry donde el facilitator aparece en credentials -> DEBE rechazar
[ ] Test: Enviar auth entry con sub_invocations no vacias -> DEBE rechazar
[ ] Test: Enviar auth entry con SourceAccount credentials -> DEBE rechazar
[ ] Test: Enviar auth entry con function_name != "transfer" -> DEBE rechazar
[ ] Test: Enviar auth entry con args.len() != 3 -> DEBE rechazar
[ ] Test: Enviar auth entry con to != requirements.payTo -> DEBE rechazar
[ ] Test: Enviar auth entry con amount != requirements.amount -> DEBE rechazar
[ ] Test: Enviar auth entry con contract_address != requirements.asset -> DEBE rechazar

VERIFICACION DE SIMULACION (Fase 2):
[ ] Test: Verificar que simulacion exitosa emita eventos de transfer correctos
[ ] Test: Verificar que simulacion con balance changes inesperados sea rechazada
[ ] Test: Verificar que simulacion fallida retorne error apropiado

VERIFICACION DE FORMATO (Fase 3):
[ ] Test: Enviar payload formato v1 (auth entry individual) -> DEBE funcionar (legacy)
[ ] Test: Enviar payload formato v2 (transaccion completa XDR) -> DEBE funcionar
[ ] Test: Verificar x402Version=2 en /supported
[ ] Test: Verificar areFeesSponsored=true en /supported extra
[ ] Test: Verificar deteccion automatica de formato

VERIFICACION END-TO-END:
[ ] Test: Pago completo con cliente compatible con spec upstream
[ ] Test: Pago completo con cliente legacy (formato actual)
[ ] Verificar en Stellar Expert/StellarChain que la transaccion sea correcta
```

### Verificacion de Cumplimiento Sui

```
PRE-REQUISITOS:
[ ] Facilitator desplegado con Sui habilitado (--features sui)
[ ] Fondos SUI en wallet del facilitator (testnet)
[ ] Acceso a Sui RPC testnet

VERIFICACION DE SEGURIDAD (Fase 1):
[ ] Test: Verificar firma criptografica completa (no solo address match)
[ ] Test: Enviar firma invalida -> DEBE rechazar

VERIFICACION DE SIMULACION (Fase 2):
[ ] Test: Ejecutar dry_run antes del envio
[ ] Test: Verificar balance changes en efectos de dry_run
[ ] Test: Transaccion que fallaria en dry_run -> DEBE rechazar antes de envio
[ ] Test: Transaccion ya ejecutada -> DEBE detectar y rechazar

VERIFICACION DE FORMATO (Fase 3):
[ ] Test: Enviar payload formato v1 (6 campos) -> DEBE funcionar (legacy)
[ ] Test: Enviar payload formato v2 (2 campos: signature, transaction) -> DEBE funcionar
[ ] Test: Verificar que from/to/amount se extraigan de transaction BCS
[ ] Test: Verificar x402Version=2 en /supported

VERIFICACION END-TO-END:
[ ] Test: Pago sponsored completo con cliente compatible con spec
[ ] Test: Pago sponsored completo con cliente legacy
[ ] Verificar en SuiVision/SuiExplorer que la transaccion sea correcta
```

### Verificacion de Cumplimiento Algorand

```
PRE-REQUISITOS:
[ ] Facilitator desplegado con Algorand habilitado (--features algorand)
[ ] Fondos ALGO en wallet del facilitator (testnet)
[ ] Acceso a Algod testnet

VERIFICACION DE SEGURIDAD (Fase 1):
[ ] Test: Enviar amount que no coincide con requirements -> DEBE rechazar
[ ] Test: Enviar receiver que no coincide con payTo -> DEBE rechazar
[ ] Test: Enviar fee tx tipo axfer (no pay) -> DEBE rechazar
[ ] Test: Enviar fee tx con amt > 0 -> DEBE rechazar
[ ] Test: Enviar fee tx con fee > limite razonable -> DEBE rechazar
[ ] Test: Enviar grupo con 17 transacciones -> DEBE rechazar
[ ] Test: Enviar grupo con rekey_to -> DEBE rechazar
[ ] Test: Enviar grupo con close_remainder_to -> DEBE rechazar

VERIFICACION DE SIMULACION (Fase 2):
[ ] Test: Simular grupo exitoso -> DEBE pasar
[ ] Test: Simular grupo con fondos insuficientes -> DEBE fallar
[ ] Test: Simular grupo con ASA no opted-in -> DEBE fallar
[ ] Test: Verificar encoding msgpack canonico funciona con simulate API

VERIFICACION DE FORMATO (Fase 3):
[ ] Test: Verificar soporte CAIP-2 network identifier
[ ] Test: Verificar que tx_id retornado corresponde a paymentGroup[paymentIndex]

VERIFICACION END-TO-END:
[ ] Test: Pago atomico gasless completo en testnet
[ ] Test: Verificar en Allo/AlgoExplorer que ambas transacciones del grupo sean visibles
[ ] Test: Verificar instant finality (transaccion confirmada en 1 bloque)
```

### Verificacion NEAR (Auto-Auditoria)

```
PRE-REQUISITOS:
[ ] Facilitator desplegado con NEAR habilitado
[ ] Fondos NEAR en wallet del facilitator (testnet)
[ ] Acceso a NEAR RPC testnet

VERIFICACION DE SEGURIDAD:
[ ] Test: Enviar DelegateAction con sender_id=facilitator -> DEBE rechazar
[ ] Test: Enviar DelegateAction con 2 acciones (ft_transfer + add_key) -> DEBE rechazar
[ ] Test: Enviar DelegateAction con ft_transfer_call en vez de ft_transfer -> DEBE rechazar
[ ] Test: Enviar DelegateAction con amount < requirements -> DEBE rechazar
[ ] Test: Enviar DelegateAction con receiver_id != payTo -> DEBE rechazar
[ ] Test: Verificar firma invalida -> DEBE rechazar

VERIFICACION DE FEATURES UNICAS:
[ ] Test: Pago a cuenta no registrada -> facilitator debe auto-registrar
[ ] Test: Pago a cuenta ya registrada -> no llamar storage_deposit
[ ] Test: storage_deposit falla -> retornar error apropiado

VERIFICACION END-TO-END:
[ ] Test: Pago completo con meta-transaccion en testnet
[ ] Test: Verificar en NEAR Explorer que la transaccion sea correcta
[ ] Test: Verificar que el gas fue pagado por el facilitator
```

---

## Notas Finales

### Compatibilidad Backward

Todas las remediaciones DEBEN mantener compatibilidad con clientes existentes. El enfoque es:
1. **Formato dual**: Aceptar tanto formato legacy como formato spec.
2. **Deteccion automatica**: Determinar el formato del payload basado en los campos presentes.
3. **Deprecation gradual**: Marcar formatos legacy como deprecated pero funcionales.

### Orden de Prioridad de Cadenas

Basado en uso en produccion y riesgo:
1. **Stellar** - Mas gaps criticos de seguridad, implementacion mas compleja.
2. **Algorand** - Simulacion deshabilitada es un riesgo operativo alto.
3. **Sui** - Formato incompatible pero con menos gaps de seguridad.
4. **NEAR** - Sin spec upstream, pero necesita auto-auditoria de seguridad.

### Dependencias Externas

- Algorand: Resolver problema de encoding msgpack de `algonaut` para simulacion.
- Stellar: Necesitamos verificar que el SDK `stellar_xdr` soporte extraer argumentos de InvokeContractArgs correctamente.
- Sui: Verificar API de `dry_run_transaction_block` en version actual del SDK de Sui.
- NEAR: No hay dependencias externas bloqueantes.
