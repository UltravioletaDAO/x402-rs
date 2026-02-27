# 13. Evaluacion de Upgrade a Rust Edition 2024

**Fecha**: 2026-02-12
**Categoria**: Toolchain / Lenguaje
**Prioridad**: Media-Alta
**Esfuerzo estimado**: 4-8 horas (migracion) + 2-4 horas (pruebas/Docker)

---

## Resumen Ejecutivo

Upstream (`x402-rs/x402-rs`) ya migro a Rust edition 2024 con `rust-version = "1.88.0"`. Nuestro fork (`UltravioletaDAO/x402-rs`) permanece en edition 2021. Esta divergencia de edicion complica los merges de upstream y nos impide aprovechar mejoras del lenguaje. Sin embargo, la migracion tiene impacto real en nuestro codigo (especialmente `env::set_var` convertido a `unsafe` y cambios en captura de lifetimes en `impl Trait`).

**Recomendacion: Migrar a edition 2024 en el proximo ciclo de desarrollo (Q1 2026), antes del siguiente merge de upstream.**

---

## Estado Actual

### Nuestro Fork

| Componente | Valor |
|---|---|
| Edition | **2021** (todas las crates del workspace) |
| `rust-toolchain.toml` | `channel = "stable"` (sin pin de version) |
| Rust instalado (local) | **1.90.0** (2025-09-14) |
| Rust en Docker (`rust:bullseye`) | **1.93.1** (2026-02-11) |
| Archivos `.rs` | ~40 archivos fuente |
| Lineas de codigo Rust | ~28,411 |
| Tests unitarios | ~137 funciones `#[test]` |
| Workspace crates | 6 (root + 3 crates + 2 examples) |

### Upstream

| Componente | Valor |
|---|---|
| Edition | **2024** |
| `rust-version` | **1.88.0** |
| Resolver | **3** (implicito en edition 2024) |
| Workspace | Reestructurado (mas crates, diferente layout) |

### Crates del Workspace y sus Ediciones

Todas nuestras crates usan edition 2021:

```
Cargo.toml (root)              -> edition = "2021"
crates/x402-axum/Cargo.toml   -> edition = "2021"
crates/x402-compliance/Cargo.toml -> edition = "2021"
crates/x402-reqwest/Cargo.toml -> edition = "2021"
examples/x402-axum-example     -> edition = "2021"
examples/x402-reqwest-example  -> edition = "2021"
```

---

## Cambios Clave de Edition 2024

La edition 2024 fue estabilizada en Rust 1.85.0 (2025-02-20). A continuacion se detallan los cambios mas relevantes y su impacto en nuestro codigo.

### 1. `unsafe_op_in_unsafe_fn` (Warning -> Deny)

**Que cambia**: En edition 2024, las operaciones `unsafe` dentro de funciones `unsafe fn` requieren un bloque `unsafe {}` explicito. Antes, todo el cuerpo de una funcion `unsafe fn` era implicitamente un contexto unsafe.

**Impacto en nuestro codigo**: **BAJO**. No tenemos funciones `unsafe fn` definidas en nuestro codigo. Nuestros 3 usos de `unsafe` son bloques explicitos dentro de funciones seguras:
```rust
// src/from_env.rs:445
unsafe { env::set_var(self.key, value) };
// src/from_env.rs:452
Some(value) => unsafe { env::set_var(self.key, value) },
// src/from_env.rs:453
None => unsafe { env::remove_var(self.key) },
```
Estos bloques ya son correctos para edition 2024.

### 2. `env::set_var` y `env::remove_var` son `unsafe`

**Que cambia**: `std::env::set_var()` y `std::env::remove_var()` ahora son funciones `unsafe` porque modificar variables de entorno no es thread-safe.

**Impacto en nuestro codigo**: **ALTO** - Este es el cambio con mayor impacto.

**Codigo de produccion** (`src/from_env.rs`): Ya usa bloques `unsafe` correctamente (lineas 445, 452, 453). **No requiere cambios.**

**Codigo de tests** - Multiples tests usan `env::set_var`/`env::remove_var` **sin** bloques `unsafe`:

```rust
// src/escrow.rs (lineas 916-937) - 7 llamadas sin unsafe
env::remove_var("ENABLE_ESCROW");
env::set_var("ENABLE_ESCROW", "true");
env::set_var("ENABLE_ESCROW", "TRUE");
env::set_var("ENABLE_ESCROW", "1");
env::set_var("ENABLE_ESCROW", "false");
env::set_var("ENABLE_ESCROW", "0");
env::remove_var("ENABLE_ESCROW");

// src/payment_operator/mod.rs (lineas 96-117) - 7 llamadas sin unsafe
env::remove_var("ENABLE_PAYMENT_OPERATOR");
env::set_var("ENABLE_PAYMENT_OPERATOR", "true");
// ... etc.
```

**Solucion**: Envolver cada llamada en `unsafe { }` en los tests, o mejor aun, crear un helper:

```rust
/// Helper para tests: establece una variable de entorno de forma unsafe.
/// SOLO usar en tests single-threaded.
#[cfg(test)]
unsafe fn test_set_env(key: &str, value: &str) {
    unsafe { env::set_var(key, value) }
}
```

**Total de cambios necesarios**: ~14 llamadas a `env::set_var`/`env::remove_var` en tests.

### 3. Captura Implicita de Lifetimes en `impl Trait` (RPIT)

**Que cambia**: En edition 2024, los tipos `impl Trait` en posicion de retorno capturan **todos** los parametros de lifetime del scope contenedor de forma implicita (igual que los genericos de tipo). En edition 2021, los lifetimes solo se capturaban si se mencionaban explicitamente.

**Impacto en nuestro codigo**: **MEDIO**. Tenemos ~10 firmas con `-> impl Future<...> + Send` en traits:

```rust
// src/facilitator.rs
fn verify(&self, request: &VerifyRequest)
    -> impl Future<Output = Result<VerifyResponse, Self::Error>> + Send;

fn settle(&self, request: &SettleRequest)
    -> impl Future<Output = Result<SettleResponse, Self::Error>> + Send;
```

En edition 2024, el `&self` y `&VerifyRequest` serian capturados implicitamente por el `impl Future`. Esto podria causar errores de lifetime si el compilador infiere que el Future mantiene una referencia que antes no capturaba.

**Mitigacion**: `cargo fix --edition` puede agregar `+ use<'_>` para preservar el comportamiento anterior, o se puede usar `+ use<>` para indicar explicitamente que no captura lifetimes. En la practica, la mayoria de nuestras funciones async ya requieren `Send` y los lifetimes deberian ser compatibles.

### 4. Reserva de la Palabra Clave `gen`

**Que cambia**: `gen` se reserva como keyword para futuros generadores.

**Impacto en nuestro codigo**: **NINGUNO**. No usamos `gen` como identificador en ningun archivo.

### 5. Cambios en el Prelude (`Future`, `IntoFuture`)

**Que cambia**: `std::future::Future` y `std::future::IntoFuture` se agregan al prelude de edition 2024.

**Impacto en nuestro codigo**: **BAJO**. Tenemos 3 imports explicitos:

```rust
// src/chain/evm.rs:38
use std::future::{Future, IntoFuture};
// src/chain/mod.rs:1
use std::future::Future;
// src/facilitator.rs:10
use std::future::Future;
```

Estos imports se vuelven redundantes pero **no causan errores** - el compilador simplemente los ignora. `cargo fix --edition` puede eliminarlos automaticamente.

### 6. Fallback del Tipo `!` (Never Type)

**Que cambia**: Expresiones divergentes (que retornan `!`) ahora tienen fallback a `!` en vez de `()`. Ademas, el lint `never_type_fallback_flowing_into_unsafe` se convierte en `deny`.

**Impacto en nuestro codigo**: **BAJO**. No tenemos patrones conocidos que dependan del fallback `()` para el never type. La mayoria de nuestros `?` y `return Err(...)` tienen tipos concretos.

### 7. Match Ergonomics (Cambios en Binding Mode)

**Que cambia**: Las reglas para match ergonomics (auto-dereferencing en patterns) se vuelven mas estrictas. Ciertos patterns que antes compilaban pueden requerir `ref` explicito o `&` en el pattern.

**Impacto en nuestro codigo**: **BAJO**. Tenemos ~293 expresiones `match` pero la mayoria son sobre enums propios con patterns simples. `cargo fix --edition` puede resolver la mayoria de los casos automaticamente.

### 8. Scope de Temporales en Tail Expressions

**Que cambia**: Los temporales creados en la expresion final (tail expression) de un bloque ahora se destruyen al final del statement, no al final del bloque.

**Impacto en nuestro codigo**: **BAJO**. Este cambio afecta codigo que mantiene referencias a temporales en la expresion de retorno. Nuestro codigo generalmente usa `let` bindings o retornos directos.

### 9. Extension de Lifetime de Temporales

**Que cambia**: Reglas mas conservadoras para cuando el compilador extiende el lifetime de un temporal.

**Impacto en nuestro codigo**: **BAJO**. Similar al punto anterior, afecta patterns especificos que son poco comunes en nuestro codigo.

### 10. `unsafe extern` Blocks

**Que cambia**: Los bloques `extern` ahora requieren `unsafe extern` para funciones FFI, y cada declaracion requiere `safe` o `unsafe` explicito.

**Impacto en nuestro codigo**: **NINGUNO**. No tenemos bloques `extern` en nuestro codigo fuente.

### 11. `static_mut_refs` (Deny)

**Que cambia**: Referencias a `static mut` se convierten en error en edition 2024.

**Impacto en nuestro codigo**: **NINGUNO**. No usamos `static mut` en ningun archivo.

### 12. Fragmentos de Macro: `expr` en 2024

**Que cambia**: El fragmento `expr` en macros ahora incluye expresiones `const {}` y `unsafe {}` como expresiones validas (anteriormente eran statements).

**Impacto en nuestro codigo**: **BAJO**. Tenemos 2 macros (`address_evm!` y `address_sol!` en `src/types.rs`) que usan `$s:literal`, no `$e:expr`. No deberian verse afectadas.

### 13. Resolver 3 del Workspace

**Que cambia**: Edition 2024 implica resolver version 3 para workspaces de Cargo, que unifica la resolucion de features entre dependencias normales y de desarrollo.

**Impacto en nuestro codigo**: **BAJO-MEDIO**. Actualmente no especificamos resolver en nuestro `Cargo.toml`. Al cambiar a edition 2024, se usara resolver 3 automaticamente. Esto podria cambiar que features se activan para ciertas dependencias, lo que podria causar errores de compilacion sutiles o cambios en el binario final.

---

## Compatibilidad de Dependencias

### Dependencias Criticas

| Dependencia | Version | Compatible con Edition 2024? |
|---|---|---|
| `axum` | 0.8.8 | Si - activamente mantenido |
| `tokio` | 1.49.0 | Si - soporte completo |
| `alloy` | 1.0.12 | Si - crate moderno |
| `solana-sdk` | 2.3.1 | Si - pero verificar version minima de Rust |
| `sui-sdk` (git) | mainnet-v1.37.3 | Potencial problema - dependencia git puede tener requisitos propios |
| `near-*` | 0.34 | Verificar - crates menos actualizados |
| `algonaut` | 0.4 | Verificar - crate con menos mantenimiento |
| `stellar-xdr` | 21.2.0 | Verificar |
| `utoipa` | 5 | Si |
| `opentelemetry` | 0.30.0 | Si |
| `thiserror` | 2.0.18 | Si - v2 ya soporta edition 2024 |
| `serde` | 1.0.228 | Si |

### Dependencias con Riesgo

1. **`solana-sdk 2.3.1`**: El ecosistema Solana tiene historico de requerir versiones especificas de Rust. Necesita prueba de compilacion.

2. **`sui-sdk` (git dependency)**: Al depender de un tag git especifico (`mainnet-v1.37.3`), puede que esta version del SDK no compile con resolver 3 o tenga issues con edition 2024. Es la dependencia de mayor riesgo.

3. **`near-*` crates**: Ecosistema NEAR puede tener dependencias transitivas que no soporten las nuevas reglas de edition.

4. **`algonaut 0.4`**: Crate con mantenimiento irregular. Verificar si compila con las dependencias que resolver 3 seleccione.

**Nota importante**: Las crates dependientes NO necesitan ser edition 2024 para funcionar - las ediciones son interoperables. El riesgo es con resolver 3, que puede cambiar la seleccion de features.

---

## Imagen Docker Base

### Situacion Actual

```dockerfile
FROM --platform=$BUILDPLATFORM rust:bullseye AS builder
```

- **`rust:bullseye`** apunta a la ultima version de Rust con Debian 11 (Bullseye).
- Al momento de este analisis, `rust:bullseye` provee Rust **1.93.1**, que es **mas que suficiente** para edition 2024 (requiere 1.85.0+).

### Opciones de Imagen

| Imagen | Rust Version | Debian | Estado |
|---|---|---|---|
| `rust:bullseye` (actual) | 1.93.1 | Debian 11 | **Funciona** - ya soporta edition 2024 |
| `rust:bookworm` | 1.93.1 | Debian 12 | Recomendado - Debian mas reciente |
| `rust:1.88-bullseye` | 1.88.0 | Debian 11 | Minimo requerido por upstream |
| `rust:1.88-bookworm` | 1.88.0 | Debian 12 | Minimo + Debian moderno |

### Recomendacion para Docker

**No necesitamos cambiar la imagen Docker** para soportar edition 2024. Nuestra imagen actual (`rust:bullseye`) ya incluye Rust 1.93.1.

Sin embargo, recomendamos actualizar a `rust:bookworm` en el futuro cercano porque Debian Bullseye (11) esta en mantenimiento extendido (LTS hasta 2026-06-30, EOL). El cambio seria:

```dockerfile
# Antes:
FROM --platform=$BUILDPLATFORM rust:bullseye AS builder
# Despues:
FROM --platform=$BUILDPLATFORM rust:bookworm AS builder

# Y el runtime:
# Antes:
FROM --platform=$BUILDPLATFORM debian:bullseye-slim
# Despues:
FROM --platform=$BUILDPLATFORM debian:bookworm-slim
```

Este cambio de imagen base es independiente de la migracion de edition y se puede hacer en cualquier momento.

---

## Pasos de Migracion

### Fase 1: Preparacion (30 min)

1. **Crear branch de migracion**:
   ```bash
   git checkout -b feat/edition-2024-migration
   ```

2. **Verificar toolchain local**:
   ```bash
   rustc --version   # Debe ser >= 1.85.0 (tenemos 1.90.0)
   rustup update stable
   ```

3. **Compilar en edition 2021 para baseline**:
   ```bash
   cargo build --release --features solana,near,stellar,algorand,sui
   cargo test
   ```

### Fase 2: Migracion Automatica (1-2 horas)

4. **Ejecutar `cargo fix --edition`** en cada crate:
   ```bash
   # Root crate
   cargo fix --edition --features solana,near,stellar,algorand,sui

   # Workspace crates individuales
   cd crates/x402-axum && cargo fix --edition && cd ../..
   cd crates/x402-compliance && cargo fix --edition --features solana && cd ../..
   cd crates/x402-reqwest && cargo fix --edition && cd ../..
   cd examples/x402-axum-example && cargo fix --edition && cd ../..
   cd examples/x402-reqwest-example && cargo fix --edition && cd ../..
   ```

5. **Actualizar `edition` en todos los `Cargo.toml`**:
   ```toml
   edition = "2024"
   ```
   Archivos a modificar:
   - `Cargo.toml` (root)
   - `crates/x402-axum/Cargo.toml`
   - `crates/x402-compliance/Cargo.toml`
   - `crates/x402-reqwest/Cargo.toml`
   - `examples/x402-axum-example/Cargo.toml`
   - `examples/x402-reqwest-example/Cargo.toml`

6. **Opcionalmente agregar `rust-version`**:
   ```toml
   rust-version = "1.85.0"  # Minimo para edition 2024
   ```

### Fase 3: Correcciones Manuales (2-4 horas)

7. **Corregir `env::set_var`/`env::remove_var` en tests**:

   En `src/escrow.rs` (lineas 916-937), envolver en `unsafe`:
   ```rust
   #[test]
   fn test_is_escrow_enabled() {
       unsafe { env::remove_var("ENABLE_ESCROW") };
       assert!(!is_escrow_enabled());

       unsafe { env::set_var("ENABLE_ESCROW", "true") };
       assert!(is_escrow_enabled());
       // ... etc.
   }
   ```

   En `src/payment_operator/mod.rs` (lineas 96-117), mismo tratamiento.

8. **Revisar cambios de `cargo fix` en `impl Trait`**:
   - Verificar que los `+ use<>` agregados son correctos
   - Asegurar que las firmas de trait en `src/facilitator.rs` siguen funcionando
   - Probar que los `impl Future<...> + Send` en `src/chain/mod.rs` y `src/chain/evm.rs` no rompan

9. **Limpiar imports redundantes** (Future, IntoFuture del prelude):
   ```rust
   // Estos imports se pueden eliminar (ahora estan en el prelude):
   // use std::future::{Future, IntoFuture};  // src/chain/evm.rs
   // use std::future::Future;                 // src/chain/mod.rs, src/facilitator.rs
   ```

10. **Verificar macros** (`address_evm!`, `address_sol!`):
    - Usar `$s:literal` no deberia verse afectado, pero compilar para confirmar.

### Fase 4: Verificacion (2-4 horas)

11. **Compilar todo el workspace**:
    ```bash
    cargo build --release --features solana,near,stellar,algorand,sui
    ```

12. **Ejecutar todos los tests**:
    ```bash
    cargo test --features solana,near,stellar,algorand,sui
    ```

13. **Ejecutar clippy**:
    ```bash
    just clippy-all
    ```

14. **Build Docker**:
    ```bash
    ./scripts/fast-build.sh test-edition-2024
    ```

15. **Test funcional**:
    ```bash
    # Iniciar contenedor
    docker run -p 8080:8080 --env-file .env facilitator:test-edition-2024

    # Verificar endpoints
    curl http://localhost:8080/health
    curl http://localhost:8080/supported | jq '.kinds | length'
    curl http://localhost:8080/ | grep "Ultravioleta"
    ```

### Fase 5: Finalizacion (30 min)

16. **Actualizar `rust-toolchain.toml`** (opcional, para pinear version minima):
    ```toml
    [toolchain]
    channel = "stable"
    # Minimo 1.85.0 para edition 2024, pero preferimos latest stable
    ```

17. **Actualizar documentacion**:
    - `CLAUDE.md`: Cambiar referencia a edition 2021 -> 2024
    - `docs/CUSTOMIZATIONS.md`: Eliminar la seccion sobre downgrade de edition

18. **Commit y PR**.

---

## Analisis de Pros y Contras

### Pros

1. **Alineacion con upstream**: Elimina la divergencia de edition, facilitando enormemente los futuros merges de upstream. Actualmente cada merge requiere reconciliar diferencias de edition.

2. **Seguridad mejorada**: `unsafe_op_in_unsafe_fn` como deny por defecto, `env::set_var` explicitamente unsafe, y `static_mut_refs` como error previenen bugs sutiles.

3. **Mejores diagnosticos del compilador**: Las reglas mas estrictas de lifetime en `impl Trait` hacen el codigo mas predecible y los errores mas claros.

4. **Futuro del lenguaje**: Acceso a nuevas features que solo se habilitan en edition 2024+ (generadores `gen`, etc. cuando se estabilicen).

5. **Resolver 3**: Mejor resolucion de features en workspace, evitando compilaciones innecesarias de features de dev-dependencies en builds de release.

6. **Prelude expandido**: `Future` e `IntoFuture` en el prelude reduce imports boilerplate en un codebase heavy-async como el nuestro.

### Contras

1. **Riesgo con dependencias exoticas**: `sui-sdk` (git dependency), `algonaut`, y crates de NEAR/Stellar podrian tener problemas con resolver 3.

2. **Tests necesitan cambios manuales**: ~14 llamadas a `env::set_var`/`env::remove_var` en tests necesitan bloques `unsafe`.

3. **Cambios en `impl Trait` lifetimes**: Los 10 usos de `-> impl Future + Send` podrian requerir anotaciones adicionales. `cargo fix` puede no acertar en todos los casos.

4. **Incompatibilidad con Rust < 1.85**: Cualquier desarrollador con Rust antiguo no podra compilar. En la practica esto no es problema (nuestro stable es 1.90.0).

5. **Tiempo de migracion**: 4-8 horas de trabajo developer que podria dedicarse a features.

6. **Riesgo de regresion**: Aunque improbable, cambios sutiles en comportamiento de lifetimes o temporales podrian causar bugs en runtime que los tests no detecten.

---

## Evaluacion de Riesgo

| Factor | Nivel | Justificacion |
|---|---|---|
| Cambios en codigo de produccion | **Bajo** | Los `unsafe` blocks en `from_env.rs` ya son correctos |
| Cambios en tests | **Medio** | 14 llamadas a env::set_var sin unsafe |
| Compatibilidad de dependencias | **Medio** | sui-sdk y algonaut son riesgo principal |
| Docker build | **Bajo** | `rust:bullseye` ya tiene Rust 1.93.1 |
| Impacto en runtime | **Bajo** | Cambios son en su mayoria compile-time |
| Rollback | **Facil** | Revertir a edition 2021 es trivial (git revert) |
| Impacto en futuros merges upstream | **Alto positivo** | Alinear edition reduce conflictos significativamente |

**Riesgo global**: **BAJO-MEDIO**

---

## Estimacion de Esfuerzo

| Tarea | Tiempo Estimado |
|---|---|
| Preparacion y baseline | 30 min |
| `cargo fix --edition` automatico | 30 min |
| Actualizar Cargo.toml (6 archivos) | 15 min |
| Corregir env::set_var en tests | 1 hora |
| Revisar y ajustar impl Trait lifetimes | 1-2 horas |
| Compilacion completa + debugging | 1-2 horas |
| Tests unitarios | 30 min |
| Docker build y test funcional | 1 hora |
| Actualizar documentacion | 30 min |
| **Total** | **6-8 horas** |

Si las dependencias exoticas (sui-sdk, algonaut) causan problemas, agregar 2-4 horas de debugging.

---

## Recomendacion

### **MIGRAR A EDITION 2024 - Prioridad media-alta para Q1 2026**

**Justificacion**:

1. **La divergencia de edition es el mayor obstaculor para merges de upstream.** Upstream usa edition 2024 con resolver 3 y workspace-level `edition = "2024"`. Cada merge que intentemos va a requerir revertir estos cambios manualmente. Alinear la edition una vez elimina este dolor permanente.

2. **El riesgo es bajo y bien contenido.** Nuestro Rust local (1.90.0) y Docker (1.93.1) superan ampliamente el minimo de 1.85.0. Los cambios de codigo son menores: ~14 lineas de tests + revision de impl Trait lifetimes.

3. **El momento es ideal.** Tenemos Rust 1.90.0, la edition 2024 lleva un ano estabilizada (desde Rust 1.85.0, febrero 2025), y las dependencias del ecosistema han tenido tiempo de adaptarse.

4. **NO cambiar la imagen Docker base ahora.** `rust:bullseye` funciona perfectamente. El cambio a `rust:bookworm` es recomendable pero separado de la migracion de edition.

### Estrategia de Ejecucion

```
Semana 1: Ejecutar migracion en branch separado
  - cargo fix --edition
  - Corregir tests
  - Compilar y probar

Semana 2: Merge a main + build Docker de prueba
  - fast-build.sh con version de prueba
  - Test funcional completo

Semana 3: Deploy a produccion (junto con siguiente release)
```

### Si hay problemas con Sui/Algorand

Si `sui-sdk` o `algonaut` no compilan con resolver 3, opciones:

1. **Feature-gate mas agresivo**: Compilar sin `--features sui,algorand` hasta que se resuelva.
2. **Override resolver para dependencias problematicas**: Usar `[patch]` en Cargo.toml.
3. **Posponer migracion** solo para las features problematicas, manteniendo edition 2024 en el resto.

---

## Referencias

- [Rust 2024 Edition Guide](https://doc.rust-lang.org/edition-guide/rust-2024/index.html)
- [Announcing Rust 1.85.0 and Rust 2024](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/)
- [Upgrading a Large Codebase to Rust 2024](https://codeandbitters.com/rust-2024-upgrade/)
- [Transitioning to a New Edition](https://doc.rust-lang.org/edition-guide/editions/transitioning-an-existing-project-to-a-new-edition.html)
- [Docker Hub: rust official image tags](https://hub.docker.com/_/rust/tags)
