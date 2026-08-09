# ERC-8004 en Solana: qué estaba roto y qué quedó funcionando

**Fecha:** 2026-08-07
**Origen:** handoff de Execution Market (`.unused/em-8004-fix-handoff.txt`)
**Estado:** desplegado y verificado en cadena (devnet). Mainnet: lectura verificada, escritura pendiente.

---

## Resumen para quien tiene 30 segundos

El handoff de EM reportaba un solo bug chico: el PDA del config del registry.
Era correcto que ahí estaba el síntoma, pero la causa era estructural: **el módulo
`src/erc8004/solana.rs` entero estaba escrito contra una versión del programa
anterior a la v0.3.0**. Aparecieron **siete** defectos distintos, cuatro de ellos
imposibles de ver sin ejecutar contra la cadena.

Todo arreglado y desplegado entre **v1.70.0 y v1.70.3**. Los caminos de lectura y
escritura están probados con transacciones reales en devnet.

---

## Los siete defectos

| # | Qué | Cómo se encontró | Versión |
|---|---|---|---|
| 1 | Seed `["config"]` deprecada; ahora `["root_config"]` → colección → `["registry_config", collection]` | auditoría estática vs SDK | 1.70.0 |
| 2 | `RegistryConfig` declaraba `registry_type` y `base_index` inexistentes (78 B vs 73 B reales) | auditoría estática | 1.70.0 |
| 3 | `total-supply` leía `config.base_index`, campo que no existe. El registry no lleva contador | auditoría estática | 1.70.0 |
| 4 | Listas de cuentas mal en los 5 builders; hash de clave de metadata de 8 B en vez de 16 B | auditoría estática | 1.70.0 |
| 5 | `AgentAccount` con 8 campos faltantes y strings en el medio → toda lectura de agente daba 500 | **e2e en devnet** | 1.70.1 |
| 6 | `AtomStats` con 16 campos donde hay 47 → `/reputation` devolvía `atomStats: null` siempre | **e2e en devnet** | 1.70.2 |
| 7 | SEAL v1: tres funciones SHA-256 con dominios inventados. El algoritmo real es uno solo, keccak256 | **e2e en devnet** | 1.70.2 |

Más dos problemas de API que salieron por el camino:

- **`recipient` en Solana se aceptaba y se ignoraba.** La transferencia post-mint es
  solo EVM. El NFT quedaba en el facilitador y la respuesta decía `success` — quien
  integra creería que el agente se entregó. Ahora devuelve **400 explícito**.
- **`/feedback` no podía mandar `score`.** Estaba hardcodeado a `None`. Sin score, el
  ATOM Engine graba el feedback pero **no lo puntúa** (`had_impact=false`): la
  reputación quedaba en cero para siempre. Corregido en 1.70.3.

---

## Direcciones canónicas (verificadas on-chain, 2026-08-07)

### Mainnet
```
agent_registry     8oo4dC4JvBLwy5tGgiH3WwK4B9PWxL9Z4XjA2jzkQMbQ
atom_engine        AToMw53aiPQ8j7iHVb4fGt6nzUNxUhcPc3tbPBZuzVVb
root_config        FkmKMw5a8HfE733zJ1qCaLVNR7iMFFhEU5dHWxkfBCue
registry_config    BXnjUb5ZqEXovwTCrRzh6JPRxFydprLERJV2JJyCFZUz
colección          DbjsWo7iUs7QZyJxLgNyVxvAAjQZCXroJHoGok8h8Umg
authority          DVYnMNDxEHQGpyYqvkBVkXXoJ3jPWtJNAbgKNJQu1hiZ
atom_config        7mFFwuy7ryTnrRMKK246be2LwD8rgZ9DvWiZioQtuCtA
atom_cpi_authority BropKd6eEHTiTSsbecKKHr9d1zwW94BC9JXmGYg22BJx
```

### Devnet
```
agent_registry     8oo4J9tBB3Hna1jRQ3rWvJjojqM5DYTDJo5cejUuJy3C
atom_engine        AToMufS4QD6hEXvcvBDg9m1AHeCLpmZQsyfYa5h9MwAF
root_config        GGQfKNpXq8HchNxecLfXi8D7xz9PDppdPAPgr5Fx4Nvd
registry_config    Djy4TKPvFyEumcVTDCqJUHWErKqcaeRj4ULWwaPkedor
colección          6CTyGPcn8dMwKEqgtvx2XCpkGUd7uqCVK6937RSM5bhA
```

### PDA muerto — nunca inicializado
```
["config"] sobre mainnet → C9uJoGDyNQFp3gFrdYY27JsCd8utGF9TZmgH1vdzeVHL
```
Hay un test (`test_legacy_config_seed_is_not_used`) que falla si vuelve.

---

## Tres correcciones al handoff de EM

**1. Devnet SÍ está desplegado y bootstrappeado.**
El handoff decía *"el programa ni siquiera está desplegado en devnet"*. Está
desplegado, es ejecutable, tiene `root_config` inicializado y **más agentes que
mainnet**: 4.686 contra 1.391. Es el mejor sitio para probar end-to-end sin
arriesgar mainnet — de hecho es donde se validó todo esto.

**2. Las dos cuentas misteriosas.**
- `B: FkmKMw5a8HfE…` = **root_config**, seed `["root_config"]`
- `A: BXnjUb5ZqEXo…` = **registry_config**, seed `["registry_config", collection]`

El barrido de 32 seeds simples no podía encontrar A: no tiene seed estática, lleva
la colección adentro, y esa colección hay que leerla de `root_config` primero. Son
dos saltos de RPC.

**3. El layout del hexdump estaba corrido un campo.**
El handoff leyó `disc | authority | pubkey | bump` y reportó *"la misma authority
DbjsWo7i…"*. El layout real es `disc | collection | authority | bump`: `DbjsWo7i…`
es la **colección**. La authority real es `DVYnMNDxEHQGpyYqvkBVkXXoJ3jPWtJNAbgKNJQu1hiZ`.

---

## Qué debe saber Execution Market antes de integrar

### 1. `recipient` funciona en Solana desde v1.71.0

El facilitador mintea, inicializa las ATOM stats y transfiere el asset Core al
`recipient` (base58), pagando todos los fees. No hace falta que inicialicen nada.

El orden no es estético y es lo que hace que funcione: `initialize_stats` **solo
lo puede llamar el dueño**, así que ocurre antes de transferir. Al revés, esa
cuenta quedaría fuera de alcance para siempre y el agente nunca podría puntuar
reputación.

`agentWallet` **no sobrevive la transferencia** y debe re-setearlo el nuevo
dueño, igual que en EVM. Medido sobre los 6.151 agentes de mainnet y devnet:
1.909 transferidos, 175 con wallet seteada, cero con ambas.

Si el mint sale pero la transferencia falla, la respuesta es 500 con `agentId` y
`transaction` adentro: el agente existe y lo tiene el facilitador. Nunca se
reporta como entregado.

**Historial (v1.70.2 y anteriores):** el campo se aceptaba y se ignoraba en
silencio — el NFT quedaba en el facilitador y la respuesta decía `success`. Si
tienen registros de esa ventana, los agentes no se entregaron.

### 2. Manden `score` (0-100) o la reputación no existe

Sin `score`, el ATOM Engine registra el feedback en el agente pero no lo puntúa.
El programa lo dice explícito en sus logs: `had_impact=false`. Se puede acumular
feedback indefinidamente y `trustTier` seguirá en 0.

```json
POST /feedback
{ "x402Version": 1, "network": "solana",
  "feedback": { "agentId": "...", "value": 95, "score": 95, "tag1": "uptime" } }
```

### 3. El programa prohíbe auto-feedback

`SelfFeedbackNotAllowed` (error 12300). El facilitador **no puede** dejar feedback
a agentes que él mismo posee. Como los agentes registrados vía `/register` quedan
en poder del facilitador, esto importa para el diseño del flujo.

### 4. Revocar ya no exige calcular keccak256 a mano

`/feedback/revoke` acepta `originalFeedback` con el contenido del feedback original
y el facilitador deriva el SEAL hash. Los valores deben coincidir byte a byte con
la sumisión original.

```json
POST /feedback/revoke
{ "x402Version": 1, "network": "solana", "agentId": "...", "feedbackIndex": 0,
  "originalFeedback": { "value": 95, "valueDecimals": 0, "tag1": "uptime",
                        "tag2": "verify", "endpoint": "...", "feedbackUri": "..." } }
```
`sealHash: "0x…"` sigue aceptándose si lo calculan ustedes.

### 5. `totalSupply` cambió de semántica

Sale de la colección Metaplex Core, no del registry:
- `totalSupply` = `current_size` (neto de burns, equivalente a ERC-721)
- `numMinted` = acuñados desde el génesis (monótono)

---

### 6. `by-owner` responde en SVM desde v1.72.0

`GET /identity/{network}/owner/{address}` daba 200 en Base y 400 en Solana: el
handler parseaba la dirección como `Address` de EVM antes de mirar la red. Ahora
ramifica y resuelve por un `getProgramAccounts` filtrado por el discriminador de
`AgentAccount` y por el campo `owner`.

**404 y 503 son respuestas distintas y no deben colapsarse.** 404 es "esta
dirección no tiene agente"; 503 es "no pude averiguarlo", normalmente un RPC
caído, y lleva `"retryable": true`. Leer un 503 como ausencia es lo que lleva a
mintear un duplicado. Los SDKs ya lo separan por tipo: `LookupInconclusiveError`
en Python, `Erc8004LookupError.retryable` en TypeScript.

## Los dos flags de EM

Una vez confirmado lo de arriba:
- `EM_ERC8004_SOLANA_ENABLED=true`
- agregar `solana` a `REPUTATION_CAPABLE_NETWORKS`

Versiones mínimas: facilitador **v1.72.0**, SDK Python **0.42.0**, SDK
TypeScript **2.52.0**.

---

## Cómo se validó

Nada de esto se da por bueno por leer código. Cada punto tiene una transacción real
en devnet detrás.

### Devnet

```
register + transfer   agente Ec7FQVCEKme7JbFBzbh86H6ne6VrW6ZTrLsTZWGnkW9L
                      colección 4686 → 4687 minteados
                      dueño on-chain = el recipient; agent_wallet quedó en None
                      atom_stats EXISTE (antes nunca se creaba)
feedback con score    reputación pasó de ceros a qualityScore 200, lastScore 95
                      log: "AToMufS4… invoke [2] / Instruction: UpdateStats"
revoke                tx 5Q47ZZD8MLxQraCpgw9wG4rSwzM8pdUzAGCiv4sY9NENvhu9uouzuBkFXszXPQxUpdrZS823FaXjWd5gpDbtAteY
                      revokeCount 0 → 1
```

### Mainnet

```
register + transfer   agente 247Y4QLwz9ZbcuHR2nX2EQLZHCsMs1GTqvgd6fpdn85Q
                      mint     4jz6GJGQcJz2DaaDGLnhtX2M6xMgMe1NLxJE4c3tgVJrGwMqrmYyjyaGjMdAkPmZCy1TfEtNvxrXRMo46SwDuYZB
                      transfer 27VvxEGNGj4a5Z7fpCHnTFBgM84KxWRWWDvFSM5REX94FhpWShfRxdE4agAZfcaaqfNMCQTRJWvMhAz9hwDmjmC9
feedback con score    2VJc6eh9z9nM1sYvocmQd9ThvUJPj4sm6oG35tDNVk7qyFKNRDLxYA9ukA3x1h8jR4RMAMK1hD8TpwqTgsKYYCR2
                      colección 1465 → 1466; qualityScore 200, feedbackCount 1
by-owner (v1.72.0)    200 vía dueño 6xNPew…, balance 1
```

Costo del ciclo completo en mainnet: ~0.011 SOL.

### El revoke es la prueba más fuerte

El facilitador derivó el SEAL hash desde `originalFeedback` con nuestra
implementación de keccak256 y **el programa on-chain lo aceptó**. Un byte de
diferencia y lo habría rechazado. Eso valida la implementación contra el
programa mismo, no contra el SDK.

### Sobre `diversityRatio`

Devuelve `255` en ambas redes con un solo feedback, y el campo se documenta
como 0-100. Es casi seguro un centinela de "muestra insuficiente". El byte que
leemos es el correcto —el resto del struct decodifica bien— pero **no está
verificado qué significa**: no presentarlo como métrica hasta confirmarlo.

---

## Nota sobre los tests

Los tests viejos de PDA afirmaban `assert_ne!(pda, Pubkey::default())`, que pasa
igual con una seed equivocada — así sobrevivió el bug del config sin que nadie lo
viera. Y las tres funciones SEAL fabricadas pasaban porque se comparaban contra sí
mismas.

Los tests nuevos están anclados a datos externos:
- PDAs clavados a direcciones leídas de los registries vivos de mainnet y devnet.
- Layouts de cuentas fijados al tamaño exacto on-chain (`AtomStats` = 561 bytes).
- SEAL v1 fijado a **vectores generados por el SDK `8004-solana@0.8.3`**, no por
  nuestra propia implementación.
