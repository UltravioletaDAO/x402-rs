# Handoff: SKALE Programmable Privacy → Execution Market

**Date**: 2026-05-05
**From**: claude-facilitator (x402-rs)
**To**: claude-exec-market
**Priority**: P1 (interesante, diferenciador competitivo, no urgente)
**Status**: pending evaluation por EM

---

## Source

URL original que pasó el usuario:
**https://docs.skale.space/developers/programmable-privacy/intro**

El usuario (zeroxultravioleta) me preguntó si esto aplica a facilitator, EM o meshrelay. Mi conclusión: **el valor real está en EM**, no en facilitator. Por eso este handoff.

---

## Qué es SKALE Programmable Privacy (resumen técnico)

**No es TEE, no es ZK, no es FHE.** Es **threshold encryption integrado en el consenso**.

### Flujo
1. **Encrypt**: el wallet/contract cifra datos sensibles (amounts, calldata, intent) con la clave pública de umbral de SKALE.
2. **Submit**: el ciphertext se envía a un precompile o smart contract.
3. **Finalize**: el consenso finaliza el bloque sin ver el contenido.
4. **Decrypt**: post-finality, el comité de validators desencripta colectivamente usando **threshold BLS** (un mínimo de validators tiene que cooperar; ninguno solo puede desencriptar).
5. **Execute**: una vez desencriptado, ejecuta como EVM normal.

### Disponibilidad
- **SKALE Base Sepolia** (testnet): todas las features de privacy.
- **SKALE Base** (mainnet): encrypted + conditional transactions (parcial).
- **Mainnet completo**: "próximamente" (sin fecha confirmada).

### Tooling
La doc menciona compatibilidad con MetaMask, WalletConnect, Foundry, Hardhat, Ethers.js, Viem. **NO menciona SDK específico de SKALE para privacy** — la doc introductoria es liviana, faltan detalles de precompiles, gas costs y APIs concretas.

---

## Por qué creo que NO es para facilitator (x402-rs)

1. **La privacidad la da el chain, no el protocolo.** Si SKALE Base llega a mainnet con un USDC con EIP-3009, agregar SKALE al facilitator es trabajo trivial: ~155 líneas + logo + RPC en Secrets Manager. Igual que cualquier otro EVM. El facilitator firma `transferWithAuthorization` exactamente igual; la red se encarga de cifrar amounts en el explorer público.
2. **Blocker hard**: no hay confirmación pública de que SKALE Base mainnet tenga USDC nativo con EIP-3009. Sin eso, no hay x402 posible. El facilitator NO debería invertir antes de que ese requisito esté claro.
3. **Privacy de un solo settlement no aporta tanto.** Una transferencia EIP-3009 individual ya es bastante anodina en el explorer. Lo que es identificable es el patrón sostenido (mismas wallets, mismos receivers, montos consistentes) — y eso lo resuelve mejor el patrón de uso, no la red.

## Por qué NO es para meshrelay

Meshrelay es servidor IRC. Cero state on-chain. Privacy on-chain no aplica. Punto.

## Por qué SÍ es para Execution Market

EM tiene lo que el facilitator no tiene: **lógica de negocio compleja en cadena pública** — escrow, taskIds, splits agente/worker/treasury, capture/release, reputación. Eso es lo que se beneficia de privacy programable.

### Casos de uso concretos para EM en SKALE

1. **Tareas confidenciales**: clientes empresariales que NO quieren que sus competidores vean qué workers contratan ni cuánto pagan por qué tipo de tarea (pricing intelligence). Esto es objeción real en B2B.
2. **Payroll de agentes opaco**: si EM eventualmente paga a workers humanos o equipos, los amounts individuales no deberían ser doxxeables vía explorer.
3. **Subastas/oferta competitiva sin frontrunning**: bids cifrados que solo se revelan post-cierre. Esto es muy difícil de hacer bien en chains públicos sin commit-reveal complejo; SKALE lo da nativo.
4. **Reputación selectiva**: feedback ERC-8004 podría tener componente público (score agregado) y privado (detalles del trabajo) usando encrypted state.
5. **Diferenciación vs Virtuals ACP**: Virtuals está solo en Base público ($3M+ TVL, 3,400+ agentes). Si EM va a SKALE Base con privacy, tiene un argumento que ACP no puede igualar hoy.

### Costo

- Redeployar contratos de escrow en SKALE Base.
- Adaptar la lógica que actualmente lee state público (capture, release, refund, feedback) para trabajar con encrypted state donde aplique.
- El facilitator necesitaría agregar SKALE como network — pero eso solo si SKALE tiene un USDC EIP-3009 (ver "Preguntas abiertas").

---

## Tradeoffs honestos

| Pro | Con |
|---|---|
| Diferenciador competitivo fuerte vs Virtuals ACP | SKALE mainnet completo aún no está listo (sin fecha) |
| Resuelve objeción B2B real (privacy de pricing) | Doc es introductoria — faltan detalles de gas, APIs, límites |
| Threshold BLS es criptografía sólida y battle-tested | Requiere redeploy de contratos EM en otra red |
| Validator committee = no hay single point de trust | SKALE no es donde está el flujo de stablecoins (Base/Polygon/etc) |
| Stack se vuelve: x402 (pay) + ERC-8004 (trust) + ERC-8183 (work) + SKALE (privacy) | Si SKALE Base no tiene USDC EIP-3009, todo el stack se rompe |

---

## Preguntas abiertas que el equipo de EM tiene que resolver

Estas yo NO las investigué. Son las primeras cosas a chequear antes de invertir tiempo:

1. **¿SKALE Base mainnet tiene USDC nativo con EIP-3009?** Sin esto, no hay pagos x402 → no hay EM en SKALE. Esta es la pregunta blocker #1.
2. **¿Qué precompile addresses expone SKALE para encrypt/decrypt?** ¿Cómo se invocan desde Solidity? ¿Hay ejemplos canónicos en su repo?
3. **¿Cuál es el gas overhead de encrypted txns vs vanilla?** Si es 5x, los micropagos no funcionan.
4. **¿Qué partes del contract state pueden ser encrypted y cuáles no?** ¿Encrypted storage? ¿Encrypted events? ¿Encrypted calldata only? La granularidad cambia totalmente el diseño de los contratos de escrow.
5. **¿Qué pasa con la observabilidad?** Si los amounts están cifrados en explorer, ¿cómo monitoreamos health del escrow desde el facilitator/dashboards?
6. **¿Qué es exactamente "conditional transactions"?** El doc lo menciona como feature parcial en SKALE Base mainnet pero no lo explica. Puede ser relevante para release/refund de escrow.
7. **¿Cuánto cuesta SKALE en términos de fees/SKL token?** SKALE tiene economics distinta a Ethereum (subscription-based historicamente).

---

## Recomendación

**Hacer un PoC en SKALE Base Sepolia antes de comprometerse.**

Razón: el riesgo es bajo (testnet), el aprendizaje es alto (entendemos los precompiles y el modelo de encrypted state), y desbloquea la decisión real (¿vale la pena redeployar EM en SKALE mainnet cuando esté completo?).

**NO invertir en facilitator hasta que las preguntas 1 y 7 estén claras** — agregar SKALE al facilitator es trabajo barato, pero solo si tiene USDC EIP-3009 y un esquema de fees compatible con micropagos.

---

## Next steps si EM decide explorar

1. Resolver pregunta #1 (USDC EIP-3009 en SKALE Base) — esto es 1 hora de research, posiblemente ya hay un answer en su Discord/docs profundas.
2. Si #1 es positivo: clonar un contrato de escrow simple a SKALE Base Sepolia y experimentar con encrypted amount.
3. Si funciona: medir gas overhead vs Base público para una operación equivalente.
4. Si los números cierran: escribir RFC interno comparando "EM en Base público" vs "EM en SKALE Base" en términos de UX, costos y narrativa de mercado.
5. Coordinar con facilitator (yo) cuando esté listo para integrar SKALE como red soportada — el handoff inverso vendrá entonces.

---

## Coordinación cross-agent

- **claude-facilitator**: standby. Sin trabajo hasta que SKALE tenga USDC EIP-3009 confirmado.
- **claude-exec-market**: owner de evaluar este handoff. Decisión esperada: GO/NO-GO/PoC.
- **claude-sdk**: no aplica en esta fase. Si EM va a SKALE, los SDKs ya soportarían SKALE automáticamente vía CAIP-2 (`eip155:<skale-chain-id>`) sin cambios — la red es transparente para el SDK.
- **Canal IRC sugerido**: `#execution-market-facilitator` en `irc.meshrelay.xyz:6697` para discutir hallazgos.

---

## Contexto adicional (para que entiendas mi POV)

Yo soy el Claude del facilitator (x402-rs). Mi mandato es settlement multi-chain de pagos EIP-3009. Veo SKALE como "another EVM" desde mi posición — no me cambia el código si la red cifra amounts. Por eso este handoff es honesto: no estoy empujando SKALE para mi proyecto, lo estoy empujando para el tuyo porque ahí está el valor.

Si te parece que estoy equivocado y SKALE SÍ tiene un ángulo importante para el facilitator que no vi, déjamelo saber por IRC.
