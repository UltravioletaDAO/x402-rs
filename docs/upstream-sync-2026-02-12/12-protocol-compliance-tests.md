# 12. Protocol Compliance Test Harness

**Fecha**: 2026-02-12
**Componente upstream**: `protocol-compliance/`
**Tipo**: Infraestructura de testing cross-language
**Prioridad de adopcion**: Media-Alta

---

## Resumen de la Feature

Upstream construyo un directorio `protocol-compliance/` con un harness de testing end-to-end que valida interoperabilidad entre implementaciones del protocolo x402. El harness:

- Levanta binarios reales (facilitator Rust, server Rust, server TypeScript)
- Ejecuta clientes reales (Rust CLI, TypeScript fetch)
- Hace pagos reales en testnets (Base Sepolia para EIP-155, Solana Devnet)
- Verifica el flujo completo: 402 sin pago -> 200 con pago
- Usa **Vitest** como test runner y **TypeScript** como lenguaje orquestador

El objetivo principal es verificar que las implementaciones Rust y TypeScript del protocolo x402 son **interoperables** en todas las combinaciones posibles de client/server/facilitator.

---

## Implementacion Upstream

### Arquitectura

```
protocol-compliance/
├── src/
│   ├── tests/                         # 8 archivos de test
│   │   ├── v2-eip155-exact-rs-rs-rs.test.ts
│   │   ├── v2-eip155-exact-ts-rs-rs.test.ts
│   │   ├── v2-eip155-exact-ts-ts-rs.test.ts
│   │   ├── v2-eip155-exact-rs-ts-rs.test.ts
│   │   ├── v2-solana-exact-rs-rs-rs.test.ts
│   │   ├── v2-solana-exact-ts-rs-rs.test.ts
│   │   ├── v2-solana-exact-rs-ts-rs.test.ts
│   │   └── v2-solana-exact-ts-ts-rs.test.ts
│   └── utils/
│       ├── facilitator.ts             # Spawn del binario x402-facilitator
│       ├── server.ts                  # RSServerHandle + TSServerHandle
│       ├── client.ts                  # invokeRustClient() + makeFetch()
│       ├── config.ts                  # Carga .env + genera config JSON temporal
│       ├── process-handle.ts          # Gestion de procesos hijo con logs prefijados
│       ├── waitFor.ts                 # Polling de health checks
│       └── workspace-root.ts          # Referencia al root del workspace
├── package.json                       # Vitest 4.x, @x402/* SDKs, viem, hono
├── vitest.config.ts                   # fileParallelism: false
├── tsconfig.json                      # ES2024, module ESNext
└── .env.example                       # 6 variables requeridas
```

### Como Funciona el Lifecycle de un Test

Cada archivo de test sigue este patron:

1. **`beforeAll` (120s timeout)**:
   - `RSFacilitatorHandle.spawn()`: Genera un archivo de config JSON temporal con las cadenas configuradas, lanza `target/debug/x402-facilitator --config <tempfile>`, espera health check en puerto aleatorio (via `get-port`).
   - `RSServerHandle.spawn(facilitatorUrl)` o `TSServerHandle.spawn(facilitatorUrl)`: Lanza el server apuntando al facilitator.

2. **Tests**:
   - Verifica health del facilitator (GET /health -> 200)
   - Verifica 402 sin pago (GET /static-price-v2 sin header -> 402)
   - Verifica pago exitoso (client Rust o TS hace request con pago -> 200 + "VIP content")

3. **`afterAll`**:
   - `server.stop()` -> SIGTERM
   - `facilitator.stop()` -> SIGTERM

### Diferencia Clave: Configuracion por Archivo vs Variables de Entorno

Upstream usa un sistema de configuracion por archivo JSON (`--config`):

```json
{
  "host": "0.0.0.0",
  "chains": {
    "eip155:84532": {
      "eip1559": true,
      "flashblocks": true,
      "signers": ["0xPrivateKey"],
      "rpc": [{ "http": "https://rpc...", "rate_limit": 50 }]
    },
    "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1": {
      "signer": "Base58Key",
      "rpc": "https://rpc..."
    }
  },
  "schemes": [
    { "id": "v2-eip155-exact", "chains": "eip155:*" },
    { "id": "v2-solana-exact", "chains": "solana:*" }
  ]
}
```

Nuestro facilitator usa variables de entorno (`EVM_PRIVATE_KEY_TESTNET`, `RPC_URL_BASE_SEPOLIA`, etc.) y NO tiene la flag `--config`. Esta es la **incompatibilidad mas fundamental**.

### Dependencias

| Paquete | Version | Proposito |
|---------|---------|-----------|
| `vitest` | ^4.0.18 | Test runner |
| `@x402/core` | ^2.3.1 | Tipos core del protocolo |
| `@x402/evm` | ^2.3.1 | Esquema EVM (client + server) |
| `@x402/svm` | ^2.3.0 | Esquema Solana (client + server) |
| `@x402/fetch` | ^2.3.0 | Client wrapper para fetch() |
| `@x402/hono` | ^2.3.0 | Middleware de servidor Hono |
| `viem` | ^2.45.3 | Interaccion EVM |
| `@solana/kit` | ^6.0.1 | Interaccion Solana |
| `hono` | ^4.11.9 | Server HTTP para tests TS |
| `get-port` | ^7.1.0 | Asignacion de puertos aleatorios |

---

## Matriz de Tests

### Convencion de Nombres

```
v{version}-{namespace}-{scheme}-{client}-{server}-{facilitator}.test.ts
```

### Cobertura Actual Upstream

| # | Archivo | Client | Server | Facilitator | Cadena | Verificacion |
|---|---------|--------|--------|-------------|--------|--------------|
| 1 | `v2-eip155-exact-rs-rs-rs` | Rust | Rust | Rust | Base Sepolia | Flujo completo |
| 2 | `v2-eip155-exact-ts-rs-rs` | TypeScript | Rust | Rust | Base Sepolia | Interop TS->RS |
| 3 | `v2-eip155-exact-ts-ts-rs` | TypeScript | TypeScript | Rust | Base Sepolia | Facilitator RS aislado |
| 4 | `v2-eip155-exact-rs-ts-rs` | Rust | TypeScript | Rust | Base Sepolia | Interop RS->TS |
| 5 | `v2-solana-exact-rs-rs-rs` | Rust | Rust | Rust | Solana Devnet | Flujo completo |
| 6 | `v2-solana-exact-ts-rs-rs` | TypeScript | Rust | Rust | Solana Devnet | Interop TS->RS |
| 7 | `v2-solana-exact-rs-ts-rs` | Rust | TypeScript | Rust | Solana Devnet | Interop RS->TS |
| 8 | `v2-solana-exact-ts-ts-rs` | TypeScript | TypeScript | Rust | Solana Devnet | Facilitator RS aislado |

### Extensiones Planeadas (No Implementadas)

- `siwx` - Sign-in with X
- `bazaar` - Marketplace extension
- `eip2612` - EIP-2612 Gas Sponsoring
- `erc20` - ERC20 Approval Gas Sponsoring
- Aptos chain support

---

## Nuestro Testing Actual

### Inventario de Tests

| Directorio | Archivo | Tipo | Que Prueba |
|-----------|---------|------|------------|
| `tests/integration/` | `test_facilitator.py` | Python | Importa desde karmacadabra - single request con logging |
| `tests/integration/` | `test_usdc_payment.py` | Python | Pago USDC en Base mainnet - EIP-3009 manual |
| `tests/integration/` | `test_x402_integration.py` | Python | Flujo completo x402 con GLUE token en Avalanche Fuji |
| `tests/integration/` | `test_endpoints.py` | Python | Health, /supported, /verify de facilitator + agentes Karmacadabra |
| `tests/integration/` | `test_complete_flow.py` | Python | End-to-end buyer->facilitator->seller |
| `tests/integration/` | `test_quick_payment.py` | Python | Test rapido de pago |
| `tests/integration/` | `test_payment_stress.py` | Python | Stress test de pagos |
| `tests/integration/` | `test_glue_payment.py` | Python | Pago con token GLUE |
| `tests/integration/` | `test_erc8004_feedback.py` | Python | ERC-8004 reputation feedback |
| `tests/x402/load/` | `k6_load_test.js` | k6/JS | Load test 100+ TPS |
| `tests/x402/load/` | `artillery_config.yml` | Artillery | Config de load test alternativa |
| `tests/escrow/` | 15+ archivos Python | Python | Tests de escrow (authorize, release, refund, charge) |
| `tests/escrow_integration.rs` | Rust | Rust | Unit tests de CREATE3 address y factory addresses |

### Caracteristicas de Nuestros Tests

**Fortalezas:**
- Prueban contra **produccion real** (`https://facilitator.ultravioletadao.xyz`)
- Cubren funcionalidad que upstream NO tiene (escrow, ERC-8004, GLUE token, multi-stablecoin)
- Scripts de diagnostico integrados (no solo pass/fail)
- Load testing con k6 y Artillery
- Tests de endpoints de agentes Karmacadabra

**Debilidades:**
- Son scripts **ad-hoc**, no un framework estructurado
- No hay orquestacion automatica (no se levanta/detiene el facilitator)
- Dependen de produccion o de wallets pre-fondeadas manualmente
- No verifican interoperabilidad cross-language (solo Python -> facilitator Rust)
- No hay CI/CD pipeline que los ejecute automaticamente
- Algunos scripts importan desde `karmacadabra` (dependencia externa rota)
- No prueban el protocolo v2 (CAIP-2) explicitamente

---

## Analisis de Brechas

### Lo que Upstream Prueba y Nosotros No

| Aspecto | Upstream | Nosotros |
|---------|----------|----------|
| Interoperabilidad TS client <-> Rust facilitator | Si (4 combinaciones) | No |
| Interoperabilidad Rust client <-> TS server | Si (2 combinaciones) | No |
| Protocolo v2 (CAIP-2 networks) | Si (todos los tests son v2) | No (solo v1 implicito) |
| Startup/shutdown automatico del facilitator | Si (ProcessHandle) | No (manual) |
| Asignacion de puertos aleatorios | Si (get-port) | No (hardcoded o produccion) |
| Solana end-to-end en devnet | Si (4 combinaciones) | No (solo scripts sueltos) |
| Config por archivo JSON | Si (--config) | No tenemos esa feature |

### Lo que Nosotros Probamos y Upstream No

| Aspecto | Nosotros | Upstream |
|---------|----------|----------|
| Escrow lifecycle (authorize, release, refund, charge) | Si (15+ tests) | No |
| ERC-8004 reputation feedback | Si | No |
| Multi-stablecoin (USDC, EURC, GLUE, etc.) | Si | No (solo USDC) |
| 14+ redes mainnet | Si (via produccion) | No (solo Base Sepolia + Solana Devnet) |
| Load testing (k6, 100+ TPS) | Si | No |
| Landing page y branding | Si (implicit) | No |
| AWS Secrets Manager integration | Si (produccion) | No |
| Diagnostics y troubleshooting | Si (scripts) | No |

---

## Evaluacion de Adopcion

### Podriamos Usar su Harness Directamente?

**No, no directamente.** Hay 3 incompatibilidades fundamentales:

#### 1. Estructura del Workspace Completamente Diferente

Upstream:
```
workspace members = [
  "crates/x402-types",
  "crates/x402-facilitator-local",
  "crates/chains/x402-chain-eip155",
  "crates/chains/x402-chain-solana",
  "facilitator/",                      # <-- binario separado
  "examples/x402-axum-example",
  "examples/x402-reqwest-example",
]
```

Nosotros:
```
workspace members = [
  "crates/x402-axum",
  "crates/x402-reqwest",
  "crates/x402-compliance",
  "examples/x402-axum-example",
  "examples/x402-reqwest-example",
  "."                                  # <-- facilitator es el root crate
]
```

El harness busca `target/debug/x402-facilitator` (binario del crate `facilitator/`). Nuestro binario es `target/debug/x402-rs` (root crate). **Requiere cambio en la ruta del binario.**

#### 2. Sistema de Configuracion Diferente

El harness genera un archivo JSON temporal y lo pasa con `--config`. Nuestro facilitator lee variables de entorno (`EVM_PRIVATE_KEY_TESTNET`, `RPC_URL_*`, etc.) y no acepta `--config`. **Requiere o implementar --config en nuestro facilitator, o reescribir `facilitator.ts` para pasar env vars.**

#### 3. Schemas de Configuracion Diferentes

El config upstream usa formato CAIP-2 nativo:
```json
{
  "chains": {
    "eip155:84532": { "signers": [...], "rpc": [{"http": "..."}] }
  },
  "schemes": [{ "id": "v2-eip155-exact", "chains": "eip155:*" }]
}
```

Nuestro facilitator no tiene concepto de "schemes" como objetos configurables. Las cadenas se configuran via variables de entorno individuales (`RPC_URL_BASE_SEPOLIA`).

#### Requeriria Refactoring del Workspace?

**Si, un refactoring significativo.** Para adoptar el harness tal cual:
- Separar el facilitator en su propio crate (como upstream hizo con `facilitator/`)
- Implementar `--config` para lectura de archivo JSON
- Implementar el esquema de "schemes" configurables
- Renombrar el binario a `x402-facilitator`

Esto es esencialmente **converger con el workspace upstream**, que ya se analizo como no-viable en el corto plazo debido a todas nuestras customizaciones.

---

## Alternativa: Construir Nuestro Propio Harness

### Diseno Propuesto

Un harness similar pero adaptado a nuestra arquitectura:

```
tests/compliance/
├── run.sh                             # Orquestador principal
├── config/
│   └── test.env                       # Variables de entorno para tests
├── harness.py                         # Orquestador Python (mas flexible que TS)
│   ├── spawn facilitator con env vars
│   ├── health check polling
│   └── cleanup on exit
├── test_v1_eip155.py                  # Tests protocolo v1 EIP-155
├── test_v2_eip155.py                  # Tests protocolo v2 CAIP-2
├── test_v1_solana.py                  # Tests protocolo v1 Solana
├── test_v2_solana.py                  # Tests protocolo v2 Solana
├── test_escrow_flow.py                # Tests escrow (exclusivo nuestro)
└── test_multi_stablecoin.py           # Tests EURC, AUSD, etc.
```

**Ventajas de Python sobre TypeScript para nuestro caso:**
- Ya tenemos toda la infraestructura de testing en Python
- No necesitamos los SDKs `@x402/*` de TypeScript (son los del ecosistema upstream)
- `eth_account` + `web3.py` ya cubren toda la firma EIP-3009
- No necesitamos probar interop con TS server/client (no distribuimos esos)

### Lo que Realmente Necesitamos Probar

Para nuestra situacion como **operador de facilitator** (no como **desarrollador del protocolo**):

1. **Protocolo v1 sigue funcionando** despues de cambios
2. **Protocolo v2 (CAIP-2) funciona** correctamente
3. **Todas las redes mainnet responden** en /supported
4. **Escrow lifecycle completo** funciona
5. **Multi-stablecoin** (USDC + EURC) funciona
6. **Regresiones** despues de merges upstream

NO necesitamos probar:
- Interoperabilidad TS<->Rust (no distribuimos el SDK TS)
- Combinaciones de client/server (solo operamos el facilitator)
- Aptos/nuevas cadenas que no soportamos

---

## Pros y Contras

### Opcion A: Adoptar el Harness de Upstream

| Pros | Contras |
|------|---------|
| Tests probados y mantenidos por upstream | Requiere refactoring masivo del workspace |
| Verifica interoperabilidad con SDK TS oficial | Dependencia de pnpm + Node.js + 10+ paquetes npm |
| Se mantiene sincronizado con cambios del protocolo | Config `--config` no existe en nuestro facilitator |
| 8 combinaciones de test automaticas | Solo prueba Base Sepolia + Solana Devnet (2 redes) |
| Naming convention clara y extensible | No cubre escrow, ERC-8004, multi-stablecoin |
| | No prueba nuestras 14+ redes customizadas |
| | Vitest 4.x + TypeScript es overhead para un proyecto Rust |
| | Los SDKs `@x402/*` version ^2.3.x pueden no ser compatibles con nuestro v1 |

### Opcion B: Construir Nuestro Propio Harness

| Pros | Contras |
|------|---------|
| Adaptado exactamente a nuestra arquitectura | Esfuerzo inicial de construccion |
| Prueba lo que realmente nos importa (14+ redes, escrow, multi-stablecoin) | Mantenimiento propio cuando el protocolo cambia |
| Python -- consistente con tests existentes | No verifica interop con SDK TS oficial |
| Sin dependencia de pnpm/Node.js | Podria divergir del protocolo "canonico" |
| Puede probar contra produccion y local | |
| Levanta facilitator con env vars (como funciona realmente) | |
| Se integra con nuestros scripts de diagnostico | |

### Opcion C: Hibrido -- Adaptar las Ideas, No el Codigo

| Pros | Contras |
|------|---------|
| Toma lo mejor de ambos mundos | Requiere diseno cuidadoso |
| Usa la convencion de naming de upstream | Esfuerzo moderado |
| Pero ejecuta con nuestra infraestructura Python | No es "oficialmente compatible" |
| Puede gradualmente acercarse a upstream si convergemos | |

---

## Estimacion de Esfuerzo

### Opcion A: Adoptar Harness Upstream

| Tarea | Esfuerzo |
|-------|----------|
| Implementar `--config` flag en main.rs + from_env.rs | 3-4 dias |
| Separar facilitator en crate independiente (match upstream structure) | 5-7 dias |
| Renombrar binario a `x402-facilitator` | 1 dia |
| Adaptar Dockerfile y scripts de deploy | 2 dias |
| Instalar y configurar pnpm + dependencias Node.js | 0.5 dias |
| Configurar wallets testnet y fondearlas | 0.5 dias |
| Verificar que todos los tests pasan | 2-3 dias |
| **Total** | **14-18 dias** |

### Opcion B: Construir Harness Propio

| Tarea | Esfuerzo |
|-------|----------|
| Orquestador Python (spawn/healthcheck/cleanup) | 2 dias |
| Tests protocolo v1 EIP-155 (Base, Polygon, etc.) | 1 dia |
| Tests protocolo v2 CAIP-2 | 1 dia |
| Tests Solana (mainnet + devnet) | 1 dia |
| Tests escrow lifecycle | 1 dia (ya tenemos base) |
| Tests multi-stablecoin | 0.5 dias |
| Configurar wallets testnet | 0.5 dias |
| Documentacion | 0.5 dias |
| **Total** | **7-8 dias** |

### Opcion C: Hibrido

| Tarea | Esfuerzo |
|-------|----------|
| Copiar convencion de naming y estructura de directorios | 0.5 dias |
| Orquestador Python con lifecycle management | 2 dias |
| Portar los 3 escenarios de test de upstream (health, 402, 200) | 1 dia |
| Agregar tests de escrow + multi-stablecoin + multi-red | 2 dias |
| Configurar para CI/CD futuro | 1 dia |
| **Total** | **6-7 dias** |

---

## Recomendacion

### Recomendacion: Opcion C -- Hibrido (construir propio inspirado en upstream)

**Justificacion:**

1. **No somos desarrolladores del protocolo, somos operadores.** El harness de upstream esta disenado para verificar que distintas implementaciones del protocolo (Rust vs TypeScript) son compatibles entre si. Nosotros operamos UNA implementacion (el facilitator Rust) con NUESTRAS customizaciones. Lo que necesitamos es verificar que nuestras 14+ redes, escrow, multi-stablecoin, y ERC-8004 siguen funcionando despues de cada cambio.

2. **El costo de adopcion es desproporcionado.** 14-18 dias para adoptar un harness que solo prueba 2 redes y no cubre escrow ni multi-stablecoin vs. 6-7 dias para construir algo que prueba TODO lo que nos importa.

3. **La incompatibilidad de `--config` es bloqueante.** Implementar la flag `--config` es un cambio arquitectural significativo que toca `main.rs`, `from_env.rs`, y toda la inicializacion del facilitator. No es un parche trivial.

4. **Ya tenemos la base.** Los 15+ tests de escrow, `test_usdc_payment.py`, `test_x402_integration.py`, etc. ya contienen la logica de firma EIP-3009 y comunicacion con el facilitator. Solo necesitan organizacion y un orquestador.

5. **La convergencia con upstream es un proyecto separado.** Si algun dia hacemos el refactoring del workspace para converger con upstream (separar facilitator en crate, implementar --config, etc.), en ese momento adoptar su harness sera trivial. Pero hoy no tiene sentido bloquear testing en esa convergencia.

### Plan de Accion

**Fase 1 (Semana 1): Infraestructura basica**
- Crear `tests/compliance/` con orquestador Python
- Implementar spawn/healthcheck/cleanup del facilitator local
- Portar los 3 escenarios basicos: health, 402 sin pago, 200 con pago
- Usar naming convention de upstream: `v1_eip155_exact.py`, `v2_eip155_exact.py`

**Fase 2 (Semana 2): Cobertura completa**
- Agregar tests para Solana (v1 + v2)
- Agregar tests de escrow lifecycle
- Agregar tests de multi-stablecoin (EURC en Base, EURC en Ethereum)
- Agregar test de regresion para todas las redes en /supported

**Fase 3 (Futuro): CI/CD**
- Integrar con GitHub Actions
- Tests automaticos en cada PR
- Reporte de cobertura de redes

### Que Tomar de Upstream

Aunque no adoptamos su codigo, debemos copiar estas ideas:

1. **Naming convention**: `v{version}-{namespace}-{scheme}-{pattern}` es excelente
2. **Lifecycle management**: spawn -> healthcheck -> test -> cleanup
3. **Los 3 escenarios basicos**: Cada combinacion debe probar health, 402, y 200
4. **Tests serialized** (no paralelos): Para evitar conflictos de puertos y nonces
5. **Timeout de 120s en startup**: Los binarios Rust tardan en compilar/arrancar
6. **Logging con prefijos**: `[facilitator]`, `[server]`, `[client]` -- muy util para debugging

---

## Referencias

- **Upstream README**: `git show upstream/main:protocol-compliance/README.md`
- **Config generator**: `git show upstream/main:protocol-compliance/src/utils/config.ts`
- **Process management**: `git show upstream/main:protocol-compliance/src/utils/process-handle.ts`
- **Nuestros tests escrow**: `/mnt/z/ultravioleta/dao/x402-rs/tests/escrow/`
- **Nuestros tests integration**: `/mnt/z/ultravioleta/dao/x402-rs/tests/integration/`
- **Nuestro load testing**: `/mnt/z/ultravioleta/dao/x402-rs/tests/x402/load/`
