# 11. Sistema de Configuracion JSON + CLI

**Fecha**: 2026-02-12
**Componente**: Configuracion del facilitator (carga de config, secretos, CLI)
**Impacto**: ALTO - Cambio arquitectonico fundamental en como se configura el facilitator
**Recomendacion**: NO ADOPTAR (mantener sistema actual con adaptadores selectivos)

---

## Resumen de la Funcionalidad

Upstream reemplazo completamente el sistema de configuracion basado en variables de entorno por un sistema de **archivo JSON + argumentos CLI (clap)**. El cambio introduce:

1. **`config.json`** como fuente principal de configuracion (reemplaza `.env`)
2. **CLI via clap** para especificar la ruta del archivo de config (`--config`)
3. **`LiteralOrEnv<T>`** - wrapper generico que acepta valores literales o referencias a variables de entorno (`$VAR` o `${VAR}`)
4. **Configuracion por cadena (CAIP-2)** - cada blockchain se configura individualmente con su RPC y signers
5. **`SchemeConfig`** - registro declarativo de esquemas de pago por cadena

---

## Walkthrough de la Implementacion Upstream

### Capa 1: `crates/x402-types/src/config.rs` (Nucleo generico)

Este es el corazon del sistema. Define tipos reutilizables por cualquier componente x402:

**`LiteralOrEnv<T>`** - El patron mas innovador:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralOrEnv<T>(T);
```

- Implementa `Deref<Target=T>` para acceso transparente
- En `Deserialize`, antes de parsear el valor como `T`, revisa si el string empieza con `$` o `${}`
- Si es referencia a env var, resuelve la variable y luego parsea el resultado como `T`
- Permite poner en el JSON: `"$PRIVATE_KEY"` o `"0xcafe..."` indistintamente

**`CliArgs`** (requiere feature `cli`):
```rust
#[derive(Parser)]
#[command(name = "x402-rs")]
pub struct CliArgs {
    #[arg(long, short, env = "CONFIG", default_value = "config.json")]
    pub config: PathBuf,
}
```

**`Config<TChainsConfig>`** - Generico sobre el tipo de configuracion de cadenas:
```rust
pub struct Config<TChainsConfig> {
    port: u16,          // default: $PORT o 8080
    host: IpAddr,       // default: $HOST o 0.0.0.0
    chains: TChainsConfig,
    schemes: Vec<SchemeConfig>,
}
```

- `Config::load()` parsea CLI args, lee el JSON, deserializa
- Los defaults de `port`/`host` siguen consultando env vars como fallback

### Capa 2: `facilitator/src/config.rs` (Especifico del facilitator)

Define `ChainsConfig` como un `Vec<ChainConfig>` con serializacion custom como mapa CAIP-2:

```rust
pub type Config = x402_types::config::Config<ChainsConfig>;

pub enum ChainConfig {
    Eip155(Box<Eip155ChainConfig>),
    Solana(Box<SolanaChainConfig>),
    Aptos(Box<AptosChainConfig>),
}
```

La deserializacion examina el prefijo del CAIP-2 key (`eip155:`, `solana:`, `aptos:`) para decidir que variante construir.

### Capa 3: `crates/chains/x402-chain-eip155/src/chain/config.rs` (Config por cadena EVM)

```rust
pub struct Eip155ChainConfigInner {
    pub eip1559: bool,              // default: true
    pub flashblocks: bool,          // default: false
    pub signers: Eip155SignersConfig, // Vec<LiteralOrEnv<EvmPrivateKey>>
    pub rpc: Vec<RpcConfig>,        // [{http: Url, rate_limit: Option<u32>}]
    pub receipt_timeout_secs: u64,  // default: 30
}

pub type Eip155SignersConfig = Vec<LiteralOrEnv<EvmPrivateKey>>;
```

Cada signer puede ser literal (`"0xcafe..."`) o referencia a env var (`"$HOT_WALLET_KEY"`).

### Capa 4: `config.json.example`

```json
{
  "port": 8080,
  "host": "0.0.0.0",
  "chains": {
    "eip155:84532": {
      "eip1559": true,
      "flashblocks": true,
      "signers": ["0xWALLET"],
      "rpc": [{"http": "https://rpc.com/eip155/84532", "rate_limit": 50}]
    },
    "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp": {
      "signer": "SOLANA_PRIVATE_KEY",
      "rpc": "https://rpc.com/solana/...",
      "pubsub": "wss://rpc.com/solana/..."
    }
  },
  "schemes": [
    {"id": "v1-eip155-exact", "chains": "eip155:*"},
    {"id": "v2-eip155-exact", "chains": "eip155:*"},
    {"id": "v1-solana-exact", "chains": "solana:*"},
    {"id": "v2-solana-exact", "chains": "solana:*"}
  ]
}
```

### Capa 5: `facilitator/src/run.rs` (Arranque)

```rust
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    let config = Config::load()?;
    let chain_registry = ChainRegistry::from_config(config.chains()).await?;
    let scheme_registry = SchemeRegistry::build(chain_registry, scheme_blueprints, config.schemes());
    let facilitator = FacilitatorLocal::new(scheme_registry);
    // ...server startup...
}
```

Flujo: CLI args -> JSON file -> deserialize con LiteralOrEnv -> ChainRegistry -> SchemeRegistry -> FacilitatorLocal.

---

## Nuestro Sistema Actual

### `src/from_env.rs` - Configuracion 100% por Variables de Entorno

Nuestro sistema actual es fundamentalmente diferente:

**Constantes de nombres de env vars** (~85 constantes):
```rust
pub const ENV_EVM_PRIVATE_KEY_MAINNET: &str = "EVM_PRIVATE_KEY_MAINNET";
pub const ENV_EVM_PRIVATE_KEY_TESTNET: &str = "EVM_PRIVATE_KEY_TESTNET";
pub const ENV_RPC_BASE: &str = "RPC_URL_BASE";
// ... 80+ mas para cada red y tipo de secreto
```

**`SignerType` enum** con logica mainnet/testnet:
```rust
pub fn make_evm_wallet(&self, network: Network) -> Result<EthereumWallet, ...> {
    let raw_keys = if network.is_testnet() {
        env::var(ENV_EVM_PRIVATE_KEY_TESTNET)
            .or_else(|_| env::var(ENV_EVM_PRIVATE_KEY))  // fallback generico
    } else {
        env::var(ENV_EVM_PRIVATE_KEY_MAINNET)
            .or_else(|_| env::var(ENV_EVM_PRIVATE_KEY))
    };
    // Parse comma-separated keys, build EthereumWallet
}
```

**`rpc_env_name_from_network()`** - mapeo exhaustivo Network -> env var name:
```rust
pub fn rpc_env_name_from_network(network: Network) -> &'static str {
    match network {
        Network::BaseSepolia => ENV_RPC_BASE_SEPOLIA,
        Network::Base => ENV_RPC_BASE,
        // ... 30+ redes
    }
}
```

### `src/provider_cache.rs` - Cache de Providers

```rust
pub struct ProviderCache {
    providers: HashMap<Network, NetworkProvider>,
}
// ProviderCache::from_env() -> lee env vars, crea providers para cada red
```

### `src/main.rs` - Arranque basado en env vars

```rust
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();                              // Carga .env
    let provider_cache = ProviderCache::from_env().await;  // Lee env vars
    let facilitator = FacilitatorLocal::new(provider_cache, compliance_checker);
    // ...50+ lineas de configuracion de discovery, compliance, etc...
}
```

### Integracion con AWS Secrets Manager (Produccion)

En ECS, los secretos NO van en el archivo `.env`. Van como `valueFrom` en la Task Definition de Terraform:

```json
{
  "name": "EVM_PRIVATE_KEY_MAINNET",
  "valueFrom": "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-evm-private-key-mainnet-xxxxx"
},
{
  "name": "RPC_URL_ARBITRUM",
  "valueFrom": "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-rpc-mainnet-5QJ8PN:arbitrum::"
}
```

ECS inyecta los secretos como variables de entorno al arrancar el container. El codigo Rust simplemente lee `env::var()` sin saber si el valor viene de `.env`, de ECS secrets, o de una variable de shell.

---

## Matriz de Comparacion

| Aspecto | Upstream (JSON+CLI) | Nuestro (env vars + AWS SM) |
|---------|--------------------|-----------------------------|
| **Fuente principal** | `config.json` | Variables de entorno + `.env` |
| **Secretos** | `LiteralOrEnv<T>` resuelve `$VAR` en JSON | AWS Secrets Manager -> env vars via ECS |
| **CLI args** | `--config path/to/config.json` (clap) | Ninguno (todo via env) |
| **Validacion** | Tipada en deserializacion (serde) | En runtime al crear providers |
| **Fail-fast** | Si, al cargar config | Si, en `from_env()` |
| **Hot reload** | No soportado | No soportado |
| **Redes soportadas** | Solo las definidas en config.json | Todas las del enum `Network` con env var |
| **Agregar red** | Agregar entrada al JSON | Agregar enum variant + constantes env |
| **Docker/ECS compat** | Requiere montar config.json como volumen o bake en imagen | Nativo: ECS inyecta env vars directamente |
| **Rotacion de secretos** | Editar JSON y reiniciar | Actualizar en AWS SM y reiniciar servicio |
| **Auditorias** | JSON puede contener secretos en plaintext | AWS SM tiene audit trail via CloudTrail |
| **Multi-signer** | Nativo: array de signers por cadena | Nativo: comma-separated en env var |
| **Rate limiting RPC** | Nativo: `rate_limit` por RPC endpoint | No soportado |
| **EIP-1559 toggle** | Por cadena en config | Hardcodeado en provider |
| **Flashblocks** | Por cadena en config | No soportado |
| **Receipt timeout** | Configurable por cadena | Hardcodeado |
| **Esquemas (schemes)** | Declarativos en config.json | Implicitos por NetworkFamily |
| **CAIP-2 nativo** | Si (keys del JSON son CAIP-2) | Parcial (types_v2.rs + caip2.rs) |
| **Mainnet/testnet split** | Por cadena individual | Por par de env vars (MAINNET/TESTNET) |
| **Dependencias extra** | `clap`, `serde_json` (ya tenemos) | Ninguna adicional |
| **Complejidad de merge** | Alta (reescritura completa) | N/A |

---

## Analisis Detallado: Puntos a Favor del JSON Config

### 1. Rate limiting por RPC
```json
"rpc": [{"http": "https://rpc.example.com", "rate_limit": 50}]
```
Esto es una funcionalidad real que no tenemos. Con endpoints premium de QuickNode tenemos limites de requests/segundo y actualmente no los gestionamos.

### 2. Configuracion por cadena mas granular
`eip1559`, `flashblocks`, `receipt_timeout_secs` por cadena permiten optimizar para cada red. Actualmente tenemos algunos de estos valores hardcodeados.

### 3. Multiples RPCs por cadena
El campo `rpc` es un array, permitiendo failover y balanceo de carga entre proveedores. Nosotros tenemos un solo RPC por red.

### 4. Esquemas declarativos
```json
{"id": "v2-eip155-exact", "chains": "eip155:*"}
```
Permite habilitar/deshabilitar esquemas sin recompilar. Nosotros los registramos todos implicitamente.

---

## Analisis Detallado: Puntos en Contra del JSON Config

### 1. Incompatibilidad con AWS Secrets Manager + ECS

**Este es el punto mas critico.** Nuestro stack de produccion funciona asi:

```
Terraform Task Definition
    -> secrets[].valueFrom = ARN de AWS Secrets Manager
    -> ECS inyecta como env vars al container
    -> Rust lee env::var("EVM_PRIVATE_KEY_MAINNET")
```

Con config.json tendriamos que:
- **Opcion A**: Bake el JSON en la imagen Docker -> secretos en la imagen (INACEPTABLE)
- **Opcion B**: Montar como volumen EFS -> complejidad de infraestructura adicional
- **Opcion C**: Generar el JSON al arranque desde un entrypoint script que lee Secrets Manager -> capa extra de complejidad, secretos en disco efimero del container
- **Opcion D**: Usar `LiteralOrEnv<T>` con todas las claves como `"$ENV_VAR"` -> funciona, pero entonces el JSON es solo un mapa de nombres de env vars, perdiendo toda ventaja

La opcion D es viable pero reduce el JSON a:
```json
{
  "chains": {
    "eip155:8453": {
      "signers": ["$EVM_PRIVATE_KEY_MAINNET"],
      "rpc": [{"http": "$RPC_URL_BASE"}]
    }
  }
}
```

Esto es equivalente a lo que tenemos pero con una capa de indirection extra y un archivo que mantener sincronizado.

### 2. Nuestro enum `Network` soporta 7 familias de cadenas

Upstream soporta: EVM, Solana, Aptos (3 familias).
Nosotros soportamos: EVM, Solana, NEAR, Stellar, Algorand, Sui, Fogo (7+ familias).

Migrar a su sistema de config significaria crear `ChainConfig` variants para NEAR, Stellar, Algorand, Sui, Fogo, cada uno con su propio tipo de config. Esto es una cantidad masiva de trabajo por poca ganancia.

### 3. 30+ redes requieren un config.json enorme

Tenemos ~35 variantes en el enum `Network`. Cada una necesitaria su entrada en `config.json` con RPC, signers, opciones. El archivo resultante seria de 300+ lineas de JSON, dificil de mantener y propenso a errores de sintaxis.

### 4. Rompe TODA nuestra infraestructura Terraform

La Task Definition de Terraform define secretos y variables de entorno. Tendriamos que:
- Crear un mecanismo para generar `config.json` en runtime
- O mantener un template de config.json en la imagen Docker
- Modificar el entrypoint del Dockerfile
- Ajustar IAM policies para acceso a secretos desde dentro del container (vs inyeccion por ECS)

### 5. Mainnet/testnet wallet split

Upstream configura un signer **por cadena**. Nosotros tenemos un signer **por entorno** (mainnet vs testnet). Nuestro patron:
```
EVM_PRIVATE_KEY_MAINNET -> usado para Base, Polygon, Optimism, Celo, etc.
EVM_PRIVATE_KEY_TESTNET -> usado para BaseSepolia, PolygonAmoy, etc.
```

Para replicar esto en JSON, tendriamos que repetir el mismo signer en cada entrada de cadena, o usar `"$EVM_PRIVATE_KEY_MAINNET"` en todas las cadenas mainnet (volviendo al patron de env vars).

### 6. Sin beneficio para hot reload

Ninguno de los dos sistemas soporta hot reload. Ambos requieren reinicio.

---

## Enfoque Hibrido: Que Vale la Pena Adoptar Selectivamente

En lugar de migrar al sistema JSON completo, podemos adoptar **componentes especificos** que aportan valor real:

### Adoptar: `LiteralOrEnv<T>`

El wrapper es elegante y util. Podriamos usarlo internamente sin cambiar la interfaz publica:

```rust
// Podemos adoptar el patron para futuros campos de config
// sin migrar todo el sistema
use std::str::FromStr;

pub struct LiteralOrEnv<T>(T);
// ... implementar Deserialize que resuelve $VAR
```

**Esfuerzo**: 1-2 horas. **Valor**: Bajo (ya tenemos env::var() por todos lados).

### Adoptar: Rate limiting de RPC

Implementar un rate limiter por provider es valioso independientemente del sistema de config:

```rust
pub struct RateLimitedProvider {
    inner: Provider,
    limiter: RateLimiter,  // tower::limit::RateLimit o similar
}
```

**Esfuerzo**: 4-8 horas. **Valor**: Alto para produccion con QuickNode.

### Adoptar: Config granular por cadena (parcial)

Agregar `eip1559` y `receipt_timeout_secs` configurables por red. Esto se puede hacer con un HashMap en codigo sin necesitar JSON:

```rust
// En network.rs o un nuevo chain_config.rs
impl Network {
    pub fn eip1559(&self) -> bool { ... }
    pub fn receipt_timeout_secs(&self) -> u64 { ... }
}
```

**Esfuerzo**: 2-4 horas. **Valor**: Medio.

### NO Adoptar: Archivo config.json

No vale la pena. Toda nuestra infraestructura (Terraform, ECS, AWS SM, scripts de diagnostico) asume variables de entorno.

### NO Adoptar: CLI con clap

No necesitamos argumentos CLI. El facilitator se ejecuta como servicio en ECS con configuracion via entorno. No hay casos de uso interactivos.

### NO Adoptar: SchemeConfig declarativo

Nuestros esquemas estan implicitamente definidos por las redes soportadas. No necesitamos poder deshabilitarlos selectivamente.

---

## Ruta de Migracion (Si se Decidiera Adoptar - NO RECOMENDADO)

Si alguna vez se decidiera migrar al sistema JSON, estos serian los pasos:

### Fase 1: Capa de compatibilidad (2-3 dias)
1. Crear `src/config_json.rs` con tipos compatibles con nuestro `Network` enum
2. Implementar un generador que lea env vars y produzca un `Config` struct equivalente
3. Pruebas paralelas: verificar que ambos sistemas producen el mismo ProviderCache

### Fase 2: Tipos de config por familia (3-5 dias)
1. Crear config types para NEAR, Stellar, Algorand, Sui, Fogo
2. Implementar deserializacion CAIP-2 para cada familia
3. Adaptar `ProviderCache::from_env()` para aceptar `Config` como alternativa

### Fase 3: Infraestructura Docker/ECS (2-3 dias)
1. Crear script de entrypoint que genera config.json desde env vars
2. Modificar Dockerfile para incluir entrypoint
3. Actualizar Terraform para manejar el flujo mixto
4. Pruebas de integracion en ECS

### Fase 4: Eliminacion de from_env.rs (1-2 dias)
1. Migrar todos los consumidores de env vars al nuevo Config
2. Eliminar `from_env.rs` y constantes ENV_*
3. Actualizar documentacion

**Total estimado**: 8-13 dias de trabajo
**Riesgo**: Alto (cambio de infraestructura en produccion)

---

## Pros y Contras Consolidados

### Pros de Adoptar JSON Config
- (+) Rate limiting por RPC nativo
- (+) Config granular por cadena (eip1559, flashblocks, receipt_timeout)
- (+) Multiples RPCs por cadena con failover
- (+) Esquemas habilitables/deshabilitables sin recompilar
- (+) Validacion tipada al deserializar (fail-fast mas limpio)
- (+) Alineacion con upstream para futuros merges
- (+) CAIP-2 como ciudadano de primera clase

### Contras de Adoptar JSON Config
- (-) **CRITICO**: Incompatibilidad directa con AWS Secrets Manager + ECS injection
- (-) Rompe toda la infraestructura Terraform existente
- (-) 35+ redes requieren un JSON enorme y dificil de mantener
- (-) No soporta nuestras 7 familias de cadenas (solo 3 upstream)
- (-) El patron mainnet/testnet wallet split no mapea limpiamente
- (-) 8-13 dias de esfuerzo por poca ganancia neta
- (-) Riesgo de downtime en produccion durante migracion
- (-) Los scripts de diagnostico (Python) asumen env vars
- (-) Sin hot reload, el beneficio de "config file" se reduce
- (-) `LiteralOrEnv` con todas las claves como `$VAR` pierde la razon de ser del JSON

---

## Estimacion de Esfuerzo

| Opcion | Esfuerzo | Riesgo | Valor |
|--------|----------|--------|-------|
| Adopcion completa | 8-13 dias | Alto | Bajo-Medio |
| Enfoque hibrido selectivo | 1-2 dias | Bajo | Medio |
| No adoptar (status quo) | 0 dias | Ninguno | - |
| Solo rate limiting RPC | 4-8 horas | Bajo | Alto |

---

## Recomendacion Final: NO

**NO adoptar el sistema JSON + CLI de upstream.**

### Justificacion principal:

1. **AWS Secrets Manager + ECS es superior para produccion**. La inyeccion de secretos como env vars por parte de ECS es el patron recomendado por AWS. Tiene audit trail via CloudTrail, rotacion automatizada, y no expone secretos en archivos de configuracion.

2. **El JSON no resuelve ningun problema que tengamos**. Nuestro sistema actual funciona correctamente en produccion con 35+ redes. Los unicos problemas (rate limiting, config granular) se resuelven mejor con cambios puntuales.

3. **El costo de migracion es desproporcionado**. 8-13 dias de trabajo con riesgo de downtime para lograr funcionalidad que podemos obtener en 1-2 dias con un enfoque selectivo.

4. **La divergencia con upstream es manejable**. Nuestro `from_env.rs` es un archivo auto-contenido. Los merges de upstream no tocan este archivo porque es custom nuestro. La config es la zona de mayor divergencia legitima entre un fork de produccion y el proyecto upstream.

### Acciones recomendadas:

1. **Adoptar selectivamente**: Implementar rate limiting de RPC como modulo independiente (~4-8 horas)
2. **Documentar la divergencia**: Este documento sirve como referencia de por que no migramos
3. **Monitorear upstream**: Si upstream migra a un patron compatible con secret injection (e.g., AWS SM nativo, Vault integration), reconsiderar
4. **Mantener `from_env.rs` limpio**: Seguir el patron actual de constantes + env::var() con fallbacks

---

## Referencia de Archivos

| Archivo | Descripcion |
|---------|-------------|
| `crates/x402-types/src/config.rs` (upstream) | Nucleo generico: `LiteralOrEnv<T>`, `Config<T>`, `CliArgs` |
| `facilitator/src/config.rs` (upstream) | Config especifico: `ChainsConfig`, `ChainConfig` enum |
| `crates/chains/x402-chain-eip155/src/chain/config.rs` (upstream) | Config EVM: `Eip155ChainConfigInner`, `RpcConfig`, `Eip155SignersConfig` |
| `facilitator/config.json.example` (upstream) | Ejemplo de archivo de configuracion JSON |
| `facilitator/src/run.rs` (upstream) | Arranque: `Config::load()` -> ChainRegistry -> SchemeRegistry |
| `/mnt/z/ultravioleta/dao/x402-rs/src/from_env.rs` (nuestro) | 85+ constantes ENV, `SignerType`, wallet builders |
| `/mnt/z/ultravioleta/dao/x402-rs/src/main.rs` (nuestro) | Arranque: `dotenv()` -> `ProviderCache::from_env()` |
| `/mnt/z/ultravioleta/dao/x402-rs/src/provider_cache.rs` (nuestro) | Cache de providers basado en env vars |
