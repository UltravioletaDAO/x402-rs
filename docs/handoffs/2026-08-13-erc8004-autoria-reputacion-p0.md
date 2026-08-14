# Handoff — ERC-8004: autoría de reputación secuestrada + revoke sin autenticación

**Fecha:** 2026-08-13
**Origen:** coordinación en vivo por IRC (`#agents` en MeshRelay) entre la sesión del facilitador
(`claude-x402-rs-6493d`) y la sesión de Execution Market (`claude-exec-market-c910d`).
**Estado del código al escribir esto:** nada implementado. Cero líneas tocadas. Este documento es
el plan, no un reporte de trabajo hecho.
**Última actualización:** 2026-08-14 00:06 UTC, después de que EM confirmara el reparto (§7).
**Para quién:** una sesión de Claude Code corriendo en **WSL** (en Windows este repo no compila Rust).

---

## 1. Resumen en una línea

El facilitador firma todos los feedbacks ERC-8004, así que la cadena lo registra a **él** como
autor en vez del calificador real (87,2% de los feedbacks en Base); y `/feedback/revoke` no
autentica al que llama, así que **cualquiera puede pedirnos borrar esa reputación y la firmamos**.

---

## 2. Evidencia

### 2.1 Lo que midió Execution Market (on-chain, Base)

Método: `getClients` + `getLastIndex` por cliente, sobre 40 agentes de su base de datos.

| Dato | Valor |
|---|---|
| Feedbacks atribuidos a la wallet del facilitador (`0x103040545AC5031A11E8C03dd11324C7333a13C7`) | **1384** |
| Feedbacks de raters reales | 203 |
| Proporción secuestrada | **87,2%** |
| Agentes afectados | 28 de 40 |
| Peores casos | agente `18896` → 154 nuestros vs 2 reales; agente `58517` → 109 vs 0 |

Sondeo de la implementación real detrás del proxy `0x8004BAa1…`:
impl `0x16e0fa7f7c56b9a767e34b192b51f921be31da34`, `getVersion` = 2.0.0, 18 funciones,
**cero soporte de meta-transacciones** (no existen `giveFeedbackWithSignature` / `For` /
`OnBehalf` / `BySig`, ni `trustedForwarder` ERC-2771, ni `executeMetaTransaction`, ni
`eip712Domain`, ni `multicall` — ni en bytecode ni en ABI).

> `[VERIFICADO por exec-market, no re-medido por nosotros]` — los números on-chain vienen de su
> corrida. Lo que sí verificamos nosotros es todo lo de §2.2, contra el código de este repo.

### 2.2 Lo que verificamos nosotros, contra este repo

| # | Hallazgo | Archivo:línea |
|---|---|---|
| A | `giveFeedback` **no tiene parámetro `clientAddress`**: `agentId, value, valueDecimals, tag1, tag2, endpoint, feedbackURI, feedbackHash`. El registry sólo puede usar `msg.sender`. | `src/erc8004/abi.rs:207-216` |
| B | La llamada la firma `provider.inner()`, o sea la wallet del facilitador. | `src/handlers.rs:3768` |
| C | No es el bug de una ruta: `FeedbackParams` **no tiene campo de rater**. La API no puede expresar quién califica ni aunque el cliente quisiera mandarlo. | `src/erc8004/types.rs:160-192` |
| D | Es el mismo registry que sondeó EM: `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63`. | `src/erc8004/mod.rs:84` |
| E | **`revokeFeedback` recibe sólo `agentId` + `feedbackIndex`**, firmado por nosotros. | `src/handlers.rs:4065` |
| F | **La única capa sobre `/feedback/revoke` es `require_writer_lease`**, que es un lease de concurrencia entre tasks de ECS — **no** autenticación del caller. | `src/handlers.rs:563-598` |
| G | `ProofOfPayment` sólo se **produce** en el settle y se cuelga de la respuesta; **nunca se consume**. | `src/chain/evm.rs:1377-1417`, `src/types.rs:1546` |
| H | El único `proof` en `handlers.rs` está en la **documentación** del `GET /feedback`: publicamos el shape campo por campo y lo botamos. | `src/handlers.rs:3467` |
| I | **SVM tiene el mismo defecto**: la ix declara la cuenta 0 como `[signer, writable] client (feedback author / fee payer)` y le pasamos `fee_payer = p.keypair().pubkey()`. | `src/erc8004/solana.rs:922`, `src/handlers.rs:3671` |
| J | `appendResponse` recibe `client_addr` del request y lo firma el facilitador. **No verificado** si el registry restringe el responder al dueño del agente. | `src/handlers.rs:4324-4330` |

### 2.3 La consecuencia que hay que decir en voz alta

Hallazgo E+F combinados: **hoy un POST anónimo a `/feedback/revoke` nos hace firmar el revoke de
cualquier feedback que el registry atribuya a nuestra wallet** — que es exactamente ese 87,2%.
La censura no requiere que nosotros seamos maliciosos; requiere que alguien nos lo pida.

Verificado **leyendo el código**. Deliberadamente **NO** se probó contra producción.

### 2.4 El segundo eje: quién tiene derecho a calificar

El registry no restringe a nadie: cualquier address puede calificar a cualquier `agentId`. Es
decir, **nuestra centralización está funcionando de facto como control de acceso**. Si arreglamos
la autoría sin poner un gate real, abrimos sybil.

La regla que quiere EM: *sólo califica quien pagó o cobró una tx x402 real con esa contraparte*.
Las dos mitades ya existen desconectadas:

- **Nuestra**: `ProofOfPayment` (`src/erc8004/types.rs:417`) ya viaja en la respuesta del settle
  (`src/types.rs:1546`) y `FeedbackParams.proof` dice literalmente *"required for authorized
  feedback"*. Pero no se verifica (hallazgos G y H).
- **De EM**: el gate `EM-4 enforce_counterparty_proof` está **prendido en prod** (`ecs.tf:507`) y
  resuelve el tx del payout server-side. Su hueco: `rate_worker` / `rate_agent` llaman a
  `submit_feedback` **sin pasar el proof**.

---

## 3. La propuesta de EM y nuestra auditoría

### 3.1 EIP-7702 (`FeedbackDelegate.sol`)

Ubicación (misma máquina): `Z:/ultravioleta/dao/execution-market/contracts/contracts/FeedbackDelegate.sol`
(en WSL: `/mnt/z/ultravioleta/dao/execution-market/contracts/contracts/FeedbackDelegate.sol`),
tests en `contracts/test/FeedbackDelegate.test.ts`.

Idea: el rater delega su EOA a un contrato mínimo; el facilitador manda la tx **a la dirección del
rater**; el registry observa al rater como `msg.sender` mientras nosotros pagamos el gas. No hay
que tocar el registry.

**Lo que pasa la auditoría** (leído completo, 148 líneas):
`REPUTATION_REGISTRY` es `constant`; cero `delegatecall`; cero `selfdestruct`; el `.call(data)` va
sin `value`; sólo dos selectores; el digest ata `chainid + address(this) + registry + keccak(data)
+ deadline + nonce`; nonces en storage con namespace ERC-7201.

Selectores **verificados por keccak, no por confianza**:
`giveFeedback(uint256,int128,uint8,string,string,string,string,bytes32)` → `0x3c036a7e` y
`revokeFeedback(uint256,uint64)` → `0x4ab3ca99`. Cuadran con nuestra ABI.

**Los 4 hallazgos que le pasamos** (aceptó los 4; estado al 2026-08-13 21:47 UTC):

1. ✅ **RESUELTO por EM.** Era bloqueante para poder probarlo: `REPUTATION_REGISTRY` era `constant`
   con la dirección de **mainnet**, pero nuestros 8 testnets usan otra:
   `0x8004B663056A597Dffe9eCcC1965A193B7388713` (`src/erc8004/mod.rs:91` y siguientes) — el delegate
   no se podía ensayar en ningún testnet y su primera ejecución real habría sido en mainnet. EM lo
   pasó a `immutable` por constructor (sin setter, sin upgrade), aceptando perder la misma address
   en todas las redes. **Pendiente de EM: pasarnos la address desplegada POR RED.**
2. ⚠️ **ABIERTO — y ahora es nuestro (ver P2-b).** El `receive()` payable mitiga pero **no
   garantiza** que un `transfer()`/`send()` de 2300 gas siga llegando a una EOA ya delegada.
   EM lo midió y **reportó honestamente que su test no lo prueba**: usa `hardhat_setCode`, que pone
   el code directo en la cuenta; el 7702 real guarda `0xef0100+delegate` y el EVM tiene que cargar
   el code del delegate, cobrando account-access **encima** del stipend (2600 en frío > 2300). Lo
   dejó marcado NO CONCLUYENTE en el test en vez de cantar victoria. **El reparo probablemente se
   sostiene.** Cerrarlo pide un nodo con Prague y una delegación tipo 4 de verdad, no `setCode`.
3. ✅ **RESUELTO por EM.** Agregó `cancelNonce(bytes32)` invocable sólo por la propia cuenta.
4. ✅ **Aceptado.** `relayFeedback` sigue permissionless por diseño; la mitigación es nuestra:
   emitir **deadlines cortos**.

Estado del contrato de EM: commit `d612a661`, 13/13 tests.

### 3.2 Solana: 7702 no hace falta

Lectura de EM, y es correcta: la ix **ya** declara `client` como signer; el problema es que le
pasamos nuestro keypair. En SVM múltiples firmantes por tx son nativos → **que el rater firme como
`client` y nosotros quedemos sólo como `fee_payer`**. Más simple que el camino EVM, sin hardfork y
sin delegar cuentas.

Mecánica concreta: **tx parcialmente firmada**. Nosotros la armamos, el rater firma como `client`,
nosotros firmamos como `fee_payer` y la mandamos. Ojo: eso cambia el contrato del endpoint —
`POST /feedback` deja de ser *"mandame datos"* y pasa a ser *"mandame una tx firmada"*.

### 3.3 Anclar no es verificar

> [!] **Corrección (EM tenía razón).** Una versión anterior de este documento decía que el
> proof *no puede* llegar a la cadena. **Es falso.** Campo dedicado no hay, pero `giveFeedback`
> lleva `feedbackURI` + `feedbackHash`, y ese hash es el keccak256 del documento off-chain: si
> el tx del pago va **dentro** del documento, queda anclado criptográficamente on-chain, con el
> ABI que ya existe y sin tocar el registry.

La conclusión (el gate es nuestro) se sostiene, pero por otra razón: **la cadena guarda un
hash, no comprueba lo que el documento afirma.** Nadie on-chain valida que ese tx exista, ni
que el payer sea el rater, ni que el payee sea el agente. El anclaje da **integridad**; el gate
server-side da **veracidad**. Queremos las dos.

**Cuidado al medir cobertura:** anclaje sin publicación no es prueba. Si el `feedbackURI` no es
recuperable, el hash compromete un documento que nadie puede producir y el anclaje es
decorativo. Son dos números distintos: *"lleva el tx anclado"* y *"el documento es recuperable
y su keccak cuadra con el `feedbackHash` on-chain"*.

**Dato de EM (su DB, muestreo de 400 sobre 34.171 documentos):** el **72,2%** lleva el tx del
pago anclado; el **27,8% no**. O sea 1 de cada 4 ratings se firma hoy sin prueba.

### 3.4 El anclaje es 0,0% auditable — y la causa NO es lo que parecía

EM midió la segunda métrica sobre los 30 documentos más recientes: **0,0%**. No "más bajo": cero.
El `feedbackURI` devuelve HTTP 200 con `content-type: text/html` — el SPA del dashboard, **17.811
bytes idénticos para cualquier URI**. Por eso los 30 recalculaban el mismo keccak: estaban
hasheando la misma página de React.

**Causa raíz (infra de EM):** el behavior `/feedback/*` de CloudFront (`dashboard-cdn.tf:313-326`)
vive dentro de un `dynamic` con `for_each = var.enable_evidence_pipeline ? [1] : []`, y ese flag
está en **FALSE**. El behavior no existe, las URIs caen al default behavior del SPA, y el CDN mapea
403/404 a `/index.html` con `response_code 200`. De ahí el 200 con HTML.

> ⚠️ **Segunda corrección nuestra (la inferencia estaba mal).** Dijimos que el `feedbackHash` ya
> escrito on-chain "compromete una página de React". **Falso** — era inferencia, no medición.
> EM verificó que el hash se calculó **al escribir**, sobre el JSON canónico del documento real
> (`json.dumps` con `sort_keys` y separadores sin espacios): recalculó `keccak(canónico)` contra el
> `feedback_hash` guardado en 200 documentos y **cuadran 200/200**.
>
> Consecuencia — el problema cambia de tamaño: **no es integridad perdida, es entrega apagada.**
> Arreglar el behavior `/feedback/*` **rescata los 34.171 retroactivamente**: el hash on-chain ya
> compromete el documento correcto, sólo falta servirlo.

**Eslabón que falta y es nuestro** (ver §4, ítem 6): la medición de EM cubre DB ↔ documento,
**no** documento ↔ hash on-chain. Hay que leer `readFeedback` de un par de índices y comparar el
`feedbackHash` de la cadena contra el de su DB. **Gotcha:** EM guarda el hash **sin prefijo `0x`**
(ya les costó un falso negativo) — normalizar los dos lados a minúsculas y sin prefijo.

**Directiva de Saul:** que el proof se use al máximo, que se guarde en **todos** los ratings, y que
quede también en la DB de EM, no sólo en el documento.

**Dos decisiones ya acordadas en el canal** (respuestas a las preguntas de EM):

- **Dónde vive el campo: en el SDK**, no en manos de quien arma el request. El 27,8% es la
  evidencia empírica de que un campo opcional falta 1 de cada 4 veces. Y no como parámetro que el
  caller llena: el SDK **ya recibe** el `ProofOfPayment` en la respuesta del settle, así que debe
  plomarlo solo hasta la llamada de feedback. Caller-side queda de fallback para quien no usa SDK.
  Esto es ergonomía y cobertura — **no sustituye el gate**, que sigue siendo server-side.
- **El proof viaja DOS veces:** estructurado en el request (para poder verificarlo *antes* de
  firmar) y dentro del documento (para que ancle vía `feedbackHash`). Y verificamos que las dos
  copias coincidan: `keccak(documento) == feedbackHash` y que el pago declarado en el documento sea
  el mismo del struct. Sólo el hash → no podemos verificar nada; sólo el struct → no queda anclado.
- **Si el proof no verifica: se MARCA, no se rechaza. Decisión de Saul.** Nuestra objeción era
  *"marcado dónde"*: si la marca vive en una DB o en logs, la fila on-chain queda idéntica a una
  verificada y le prestamos nuestra firma a una afirmación sin comprobar. EM la resolvió bien —
  **la marca va dentro del documento** (`proof_of_payment.status = anchored | unverified`), o sea
  **dentro del preimage del keccak que ES el `feedbackHash` on-chain**. La fila no queda idéntica:
  el hash difiere y cualquiera que resuelva el URI lo comprueba. Objeción retirada.
  **Precondición:** la marca sólo vale cuando el URI se pueda resolver → depende del P0 de EM
  (§3.4). Nuestro rollout por fases (verificar+loguear → 400) encaja sin conflicto con marcar.

---

## 4. Plan de trabajo

Tres fases **independientes entre sí** más un ítem suelto. El gate del proof cierra sybil aunque la
tx la siga firmando quien sea; el fix de autoría arregla quién figura. No hay que hacerlas juntas.

Los 6 ítems de nuestro lado, en el orden sugerido: **P0** (auth del revoke) → **P1** (gate del
proof, incluye la doble copia) → **ÍTEM 6** (cerrar la cadena on-chain, read-only) → **P2-a** (SVM)
→ **P2-b** (spike tipo 4 + H2, y después la integración 7702 cuando EM despliegue). El reparto
completo y confirmado con EM está en §7.

### Regla de oro para la sesión que ejecute esto

- **NO pushear.** Commits locales sí; `git push` sólo con OK explícito de Saul, por push.
  Recordá que en este repo **push a `main` = deploy a producción** (`.github/workflows/ci.yaml`).
- **NO desplegar, NO correr terraform, NO tocar AWS.**
- **NO probar el revoke contra producción.** Es destructivo e irreversible.
- **NO tocar los 1384 feedbacks históricos.** Decisión pendiente de Saul (§6).
- Sin emojis en código Rust (`[OK]` / `[FAIL]` / `[WARN]`).

---

### FASE P0 — Cerrar `/feedback/revoke` (no depende de nadie externo)

**Por qué primero:** es la única de las tres que es una vulnerabilidad viva y explotable hoy por
un tercero anónimo, y no depende ni de EM ni de un contrato desplegado.

**Diseño:** reutilizar el patrón de admin que YA existe y ya fue revisado en este repo —
`admin_auth()` + `admin_reject()` (`src/handlers.rs:704-748`), que compara en tiempo constante y
responde 404 cuando el token no está configurado (la ruta se vuelve indistinguible de inexistente).

**Decisión de diseño tomada:** usar una variable de entorno **nueva y propia**,
`ERC8004_ADMIN_TOKEN`, y **no** reutilizar `BAZAAR_ADMIN_TOKEN`: son radios de explosión distintos
(uno esconde un listing del bazaar, el otro destruye reputación on-chain de terceros).
Fail-closed: sin la variable → 404, o sea al desplegar esto el revoke queda apagado hasta que
alguien ponga el secreto a propósito.

**Pasos:**

1. Generalizar `admin_auth()` para que tome el nombre de la env var como parámetro
   (hoy tiene `"BAZAAR_ADMIN_TOKEN"` hardcodeado en `src/handlers.rs:714`), sin cambiar el
   comportamiento del bazaar.
   `verify:` `cargo test -p x402-rs -- --test-threads=1` sigue verde y el bazaar admin se comporta igual.
2. Separar `/feedback/revoke` de `erc8004_write_routes()` (`src/handlers.rs:584-598`) a su propio
   `Router` con **dos** capas: `require_writer_lease` **y** el nuevo gate admin; mergearlo de
   vuelta para que la ruta pública no cambie de path.
3. Test nuevo: sin header → 404; con token errado → 401; con token correcto → pasa al handler.
   `verify:` los tres casos en el suite.
4. **Corrección de verdad-en-publicidad** (cuesta nada y hoy engañamos): en el `GET /feedback`
   (`src/handlers.rs:3467`) el `proof` se anuncia campo por campo como si lo validáramos.
   Mientras P1 no esté, marcarlo explícitamente como *no verificado todavía*. Misma corrección en
   `src/openapi.rs` donde aplique.
5. `src/openapi.rs`: documentar que `POST /feedback/revoke` ahora requiere
   `Authorization: Bearer <ERC8004_ADMIN_TOKEN>` y que responde 404 cuando no está configurado
   (mismo texto que ya usan las rutas admin del bazaar, `src/openapi.rs:86`).
6. `.env.example`: agregar `ERC8004_ADMIN_TOKEN=` vacío, con comentario de que en producción sale
   de AWS Secrets Manager. **No** poner ningún valor.

**Definición de listo (P0):**
- `just format-all` y `just clippy-all` limpios.
- `cargo clippy -p x402-compliance` limpio.
- `cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1` verde.
- `cargo test -p x402-compliance -- --test-threads=1` verde.
- Los 3 casos de auth del paso 3 pasan.
- Commit local hecho, **sin pushear**.

---

### FASE P1 — Gate del `proof` (anti-sybil), server-side

**Leer §3.3 y §3.4 antes de empezar esta fase** — ahí está por qué el gate tiene que ser nuestro
(anclar da integridad, verificar da veracidad), la decisión de que el proof viaja dos veces, y la
decisión de Saul de marcar en vez de rechazar.

**Dependencia interna que hay que resolver primero:** hoy la request no dice quién califica
(hallazgo C), así que *"payer == quien califica"* es incomprobable. Por eso P1 **incluye** agregar
el campo del rater, aunque la autoría real recién se arregle en P2.

**Pasos:**

1. Agregar `rater: Option<MixedAddress>` a `FeedbackParams` (`src/erc8004/types.rs:160`).
2. Escribir `verify_proof_of_payment(provider, &proof, rater, agent_id)` con estas comprobaciones
   (todas necesarias, ninguna suficiente sola):
   - `proof.network` == la red de la request.
   - la tx existe en esa red y su receipt es exitoso.
   - `receipt.block_number` == `proof.block_number`.
   - el log `Transfer` del token `proof.token` tiene `from == proof.payer`, `to == proof.payee`,
     `value == proof.amount`.
   - `proof.payer == rater`.
   - `proof.payee` == el dueño/wallet del agente. **[DECIDIR CONTRA LA CADENA]** si es
     `getAgentWallet(agentId)` (`src/erc8004/abi.rs:93`) o `ownerOf(agentId)` (`:126`) — no asumir,
     medirlo contra un agente real antes de fijarlo.
   - frescura: `now - proof.timestamp <= ERC8004_PROOF_MAX_AGE_SECS` (default sugerido: 7 días).
   - `payment_hash` recomputado con `ProofOfPayment::compute_payment_hash` y comparado.
   - **coherencia de las dos copias** (ver §3.3): si viene `feedbackHash` y el documento es
     recuperable por `feedbackURI`, verificar `keccak(documento) == feedbackHash` y que el pago
     declarado adentro sea el mismo del struct. Un documento no recuperable **no** invalida el
     feedback por sí solo, pero se registra como anclaje no auditable.
   - **anti-replay**: un pago no puede rendir 50 calificaciones. Reutilizar `src/nonce_store.rs`
     (ya es un store DynamoDB con helpers de clave por cadena) con clave
     `erc8004-proof:<network>:<txhash>:<agentId>`.
3. Rollout en dos tiempos con `ERC8004_REQUIRE_PROOF` (default `false`):
   - `false` → verifica igual y **loguea** el veredicto sin rechazar (para medir cuánto tráfico
     real se rompería). Esto es lo que le da tiempo a EM a arreglar su hueco de
     `rate_worker`/`rate_agent`.
   - `true` → rechaza con 400 y un motivo acotado (**nunca** el error crudo: puede traer
     direcciones y URLs de RPC con keys — ver `src/redact.rs`).
   - **Ortogonal al marcado:** por decisión de Saul, un proof que no verifica se **marca** en el
     documento (`proof_of_payment.status`), lo escriba quien lo escriba. Las fases de arriba
     controlan si además lo **rechazamos**; no reemplazan la marca.
4. Revertir la corrección de §P0.4: cuando el gate esté prendido, la doc vuelve a decir que sí se
   verifica, y ahora es cierto.

**Definición de listo (P1):** mismos 4 comandos verdes que P0, más tests unitarios de
`verify_proof_of_payment` que cubran cada rechazo por separado (red equivocada, tx inexistente,
monto distinto, payer distinto del rater, proof viejo, hash recomputado que no cuadra, replay).

---

### FASE P2 — Autoría real (quién figura como autor)

**Orden sugerido: Solana primero.** Es más simple, no necesita hardfork, no necesita delegar
cuentas de terceros y no depende del contrato de EM.

**P2-a — SVM:**
- Construir la ix con `client = <pubkey del rater>` en vez de `fee_payer`
  (`src/handlers.rs:3671`, `src/erc8004/solana.rs:931`).
- Flujo de tx parcialmente firmada: endpoint que devuelve la tx sin firmar → el rater firma como
  `client` → nosotros firmamos como `fee_payer` y la mandamos.
- Compatibilidad: el camino viejo queda detrás de un flag y marcado como deprecado, porque cambia
  el contrato del endpoint.

**P2-b — EVM (EIP-7702):** *parcialmente bloqueado por EM* — falta que nos pase la address del
delegate desplegado **por red** (H1 ya lo hizo `immutable`, así que la address cambia por cadena).

**Spike que SÍ se puede hacer ya, y que además cierra H2** (nos lo llevamos nosotros, acordado en
el canal): levantar `anvil --hardfork prague`, hacer una delegación **tipo 4 de verdad** (no
`hardhat_setCode`) y medir un `transfer()` de 2300 gas contra la EOA delegada. Sale gratis porque
construir tx tipo 4 con alloy es exactamente el trabajo que P2-b necesita igual; el número de H2
cae de yapa. **Ese número decide cómo se le redacta el consentimiento al rater**: si el transfer
pasa, el costo es "te ven code en la cuenta"; si falla, el costo es "hay wallets y contratos que ya
no te pueden mandar ETH", que es otra conversación y no se puede vender igual.

- `alloy 1.7.3` (ya en `Cargo.lock`) soporta tx tipo 4; hoy **no** usamos `authorization_list` en
  ninguna parte del código.
- Trabajo nuestro: armar la tx tipo 4 con la authorization firmada por el rater, mandarla **a la
  dirección del rater**, llamando `relayFeedback(data, deadline, nonce, signature)`.
- **Capacidad por red, con fallback.** 7702 pide Pectra: Base sí, pero servimos ERC-8004 en 20
  redes y varias no la tienen. El fallback ya existe del lado de EM (`prepare-feedback`: el rater
  firma con su wallet y paga su gas — más caro pero correcto).
- Emitir **deadlines cortos** en el digest (mitigación del hallazgo 4 de la auditoría).

**P2-c — pendiente de verificar:** `appendResponse` (hallazgo J). Averiguar si el registry
restringe el responder al dueño del agente. Si no lo restringe, tiene el mismo problema de
suplantación y necesita el mismo tratamiento.

---

### ÍTEM 6 — Cerrar la cadena DB → documento → on-chain (read-only, se puede hacer ya)

Nos lo comprometimos con EM en el canal. Su medición cubre **DB ↔ documento** (200/200); falta el
tramo **documento ↔ hash on-chain**.

**Pasos:**
1. Pedirle a EM (o sacar de su DB) 2-3 tuplas `(agentId, clientAddress, feedbackIndex,
   feedback_hash, feedbackURI)` de feedbacks reales en Base.
2. Llamar la función de lectura del `ReputationRegistry` para esos índices (ver `src/erc8004/abi.rs`,
   la familia de lectura alrededor de `:263-308` que toma `clientAddress`) contra
   `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` en Base.
3. Comparar el `feedbackHash` on-chain contra el de su DB.
   **Gotcha obligatorio:** EM guarda el hash **sin prefijo `0x`**. Normalizar ambos lados a
   minúsculas y sin prefijo antes de comparar, o se produce un falso negativo (ya les pasó).
4. Reportar el resultado en `#agents` con los `agentId` e índices exactos usados.

`verify:` los 2-3 hashes coinciden (o, si no coinciden, ESO es el hallazgo y hay que reportarlo tal
cual, no explicarlo).

**Es read-only.** No firma nada, no gasta gas, no toca producción. Un `eth_call` contra un RPC
público de Base alcanza.

---

## 5. Orden de ejecución y qué está bloqueado

| Fase | Depende de | Se puede empezar ya |
|---|---|---|
| P0 revoke auth | nadie | **Sí** |
| P1 gate del proof | nadie (P1 trae su propio campo `rater`) | **Sí** |
| P2-a SVM autoría | nadie | Sí |
| P2-b spike tipo 4 + H2 (anvil prague) | nadie | **Sí** |
| P2-b integración EVM 7702 | EM: address del delegate desplegado (y ese deploy espera OK de Saul) | **No** |
| P2-c appendResponse | verificación on-chain del registry | Sí (es investigación) |
| Ítem 6: cerrar la cadena hasta on-chain | EM: 2-3 tuplas de ejemplo (o su DB) | Sí, en cuanto lleguen |

---

## 6. Decisiones que son de Saul, no nuestras

1. ⏳ **Los 1384 feedbacks históricos.** Las dos sesiones coinciden en **NO tocarlos**: limpiarlos
   con `revoke` sería estrenar exactamente el poder del que nos queremos deshacer. Falta que Saul
   lo confirme. **Matiz nuevo (§3.4):** si el CDN de EM los rescata, dejan de ser basura
   inauditable y pasan a ser **historia legible con autoría equivocada** — es otro objeto y puede
   cambiar la decisión.
2. ⏳ **¿`/feedback/revoke` queda admin-only para siempre**, o se reabre cuando la autoría esté
   arreglada y el rater pueda firmar su propio revoke vía delegate?
3. ✅ **RESUELTA: se marca, no se rechaza.** Saul decidió marcar. La marca va dentro del documento
   (`proof_of_payment.status`), que es el preimage del `feedbackHash` on-chain. El rollout en dos
   fases de `ERC8004_REQUIRE_PROOF` sigue vigente y es ortogonal (§P1.3).
4. ⏳ **El deploy del `FeedbackDelegate`** (lado EM) es un write on-chain y espera **OK explícito de
   Saul**. EM no lo hace por su cuenta.

---

## 7. El reparto, confirmado por las dos sesiones

EM lo confirmó textual en `#agents`: *"[AGREE] Tu reparto 5 y 5 me cuadra"*, con dos ajustes suyos
(ya incorporados abajo). Esto **no** es una lista que nos inventamos: está acordada.

**Nuestro lado (x402-rs) — 6 ítems:**

| # | Qué | Dónde en este doc |
|---|---|---|
| 1 | Autenticar `POST /feedback/revoke` | §4 FASE P0 |
| 2 | Gate del proof server-side + anti-replay, en dos fases | §4 FASE P1 |
| 3 | Verificar la doble copia: struct vs documento, `keccak(doc) == feedbackHash` | §4 FASE P1, paso 2 |
| 4 | Autoría en SVM vía tx parcialmente firmada | §4 FASE P2-a |
| 5 | Spike de tx tipo 4 con `anvil --hardfork prague` — cierra H2 | §4 FASE P2-b |
| 6 | Cerrar la cadena DB → documento → on-chain (read-only) | §4 ÍTEM 6 |

**Lado de Execution Market — 5 ítems (no son nuestros, están acá para saber qué esperar):**

| # | Qué | Estado |
|---|---|---|
| 1 | El behavior `/feedback/*` de CloudFront — **su P0**, sin eso lo nuestro ancla algo ilegible | Arrancando |
| 2 | El proof al SDK. **Ajuste de Saul:** va en **los dos** — `uvd-x402-sdk` (transporte hacia nosotros) y `em-plugin-sdk` (para que los plugins no armen el request a mano). Adoptó nuestra idea de plomarlo del settle al feedback en vez de un parámetro opcional | Acordado |
| 3 | Cerrar el hueco de `rate_worker` / `rate_agent`, que llaman `submit_feedback` sin proof | Pendiente |
| 4 | Desplegar el `FeedbackDelegate` corregido y pasarnos la address **por red** | **Bloqueado**: el deploy es write on-chain y espera OK explícito de Saul |
| 5 | Re-medir cobertura con las dos métricas separadas, cuando el CDN esté arreglado | Pendiente |

Ya entregado por EM: H1 (`immutable` por constructor), H3 (`cancelNonce`), H4 (aceptado);
commit `d612a661`, 13/13 tests. H2 quedó marcado NO CONCLUYENTE y **es nuestro** (ítem 5).

---

## 8. Contexto de la conversación (para no repetirla)

- Canal: `#agents` en `irc.meshrelay.xyz:6697`. Historial público:
  `https://api.meshrelay.xyz/irc/channels/%23agents/messages?limit=100` (tope 100, sin paginación;
  se pierde rápido con el tráfico de la flota `kk-*`).
- La sesión de EM cambia de nick por sesión (`claude-exec-market-<hash5>`): fue `9a885`, `b2763`,
  `c910d`. No asumir que un nick sigue vivo.
- Su daemon puede estar `connected` mientras su sesión no lee: los mensajes le llegan al inbox y
  se quedan ahí hasta que corra `read --new`. Si no contesta, no está caído — está dormida.
- EM se escaló el hallazgo del revoke a Saul de su lado.
