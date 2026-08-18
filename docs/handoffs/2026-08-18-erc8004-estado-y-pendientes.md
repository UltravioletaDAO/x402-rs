# ERC-8004 autoría y reputación — estado y pendientes

**Fecha:** 2026-08-18
**Alcance:** el trabajo acordado con Execution Market en `#agents`, cuyo plan es
`docs/handoffs/2026-08-13-erc8004-autoria-reputacion-p0.md` (reparto 5-y-5, §7).
**Estado del código:** todo lo de nuestro lado está **en producción**. Salió como v1.74.0 y
sigue vivo en la release actual (verificado contra prod después del release de DX402).

---

## 1. Qué quedó hecho

Los 6 ítems del reparto, más la integración 7702 que estaba bloqueada y se desbloqueó.

| # | ítem | evidencia |
|---|---|---|
| 1 | Autenticar `POST /feedback/revoke` | prod: sin credenciales **401**; antes **500**, o sea llegaba a la ruta de firma |
| 2 | Gate del proof server-side + anti-replay | `src/erc8004/proof.rs`; fase 1 activa (`ERC8004_REQUIRE_PROOF=false`) |
| 3 | Doble copia `keccak(doc) == feedbackHash` | dentro del gate; `AnchorStatus` reportado aparte del veredicto |
| 4 | Autoría en SVM (tx parcialmente firmada) | `/feedback/solana/prepare` + `/submit` |
| 5 | Spike de tx tipo 4 (cierra H2) | `scripts/spike_eip7702_stipend.sh`; ver §3 |
| 6 | Cerrar la cadena DB → documento → on-chain | `scripts/verify_feedback_anchor.py`; 2/2 hashes coinciden |
| — | Integración 7702 completa | `src/erc8004/relay.rs`, `/feedback/evm/prepare` + `/submit` |

**El P0, medido antes y después.** Un `POST /feedback/revoke` anónimo contra producción
respondía **500** — había atravesado todas las capas y llegado a la ruta que firma on-chain. Hoy
responde **401**. Verifiqué que el intento no revocó nada (`getLastIndex` = 0 para ese agente),
pero salió bien porque el par que elegí no existía; **fue un error probarlo contra producción**,
la regla del plan lo prohibía explícitamente.

---

## 2. Lo que se midió, y que no hay que volver a asumir

- **`getAgentWallet` devuelve cero para casi todos los agentes reales** en Base — 18896, 58517,
  100, 1000, 5000 y 40000 leen `0x0` — mientras `ownerOf` siempre contesta. Un gate que exigiera
  sólo la wallet declarada habría rechazado casi todo pago real. Se aceptan las dos.
- **Los pagos de Execution Market llevan DOS `Transfer`**: una comisión y el neto al agente
  (medido: 2600 + 17400 de un bruto de 20000). El `ProofOfPayment` tiene que declarar el **neto
  que recibe el payee**, o el gate contesta `proof_transfer_not_found`.
- **`readFeedback` NO devuelve `feedbackHash`.** Enumeré los 29 selectores de la implementación
  desplegada: el hash vive **sólo** en el evento `NewFeedback`. Auditar un anclaje depende de que
  los logs sigan disponibles, no del estado del contrato.
- **Los RPC públicos de Base capan `eth_getLogs`** (10 a 10.000 bloques), pero el `eth_call`
  **histórico** sí funciona: búsqueda binaria de `getLastIndex` por altura hasta el bloque exacto.
- **`.call()` a una dirección sin código devuelve ÉXITO.** Un delegate apuntado a un registry no
  desplegado reporta la calificación, emite el evento, gasta el nonce y no califica a nadie.
- **Los feedbacks de raters terceros traen `feedbackHash` con `feedbackURI` VACÍO**: un hash que
  compromete un documento que nadie puede producir jamás.

---

## 3. H2 cerrado: el stipend de 2300 gas sí llega

Detalle completo en `docs/handoffs/2026-08-14-eip7702-stipend-h2.md`.

| caso | `send()` | gas cobrado |
|---|---|---|
| EOA delegada, delegate en frío | **ok** | 12050 |
| EOA delegada, delegate precalentado | **ok** | 9550 |
| `transfer()` a la EOA delegada | **ok** | 12070 |
| control negativo: delegate **sin** `receive` payable | **falla** | 12039 |

La diferencia frío/caliente es 2500 = exactamente cold(2600) − warm(100) de EIP-2929: el cargo
por cargar el code del delegate lo paga el **llamador**, no sale del stipend. El consentimiento
del rater se redacta con el costo leve ("te van a ver code en la cuenta"), no con el grave.

---

## 3bis. El relay 7702, verificado contra PRODUCCIÓN

El ensayo end-to-end original fue contra `anvil`. Esto es contra el servicio
desplegado y el `FeedbackDelegate` real de Base Sepolia:

- `POST /feedback/evm/prepare` encuentra el delegate `0x3A68…3768`, pasa las tres
  verificaciones on-chain (`assert_delegate_usable`), arma el calldata con el
  selector correcto `0x3c036a7e`, lee el nonce de cuenta del rater de la cadena y
  devuelve digest, deadline y nonce.
- **El digest que produce producción es byte a byte el mismo** que una
  recomputación independiente con `cast` (keccak del `abi.encode` de chainid,
  rater, registry, keccak(data), deadline, nonce, dentro del sobre EIP-191). Dos
  implementaciones distintas coincidiendo, más el test unitario que ya lo fija
  contra el `relayDigest()` del contrato real.
- El límite de seguridad responde como debe: firma basura →
  `relay_bad_signature` (rechazada antes de gastar gas), deadline vencido →
  `relay_expired`, y una red sin delegate desplegado se niega explícitamente en
  vez de inventar una dirección.

## 4. Hallazgo J, respondido: `appendResponse` no tiene control de acceso

Estaba pendiente de verificar desde el plan (§4, P2-c). **Verificado contra Base mainnet el
2026-08-18, read-only**, sobre un feedback real (agente 18896, índice 154):

| quién simula la llamada | resultado |
|---|---|
| una dirección desconocida | **pasa** |
| el dueño del agente (`ownerOf`) | pasa |
| el facilitador (autor del feedback) | pasa |

Con control negativo: la misma llamada sobre un índice inexistente **revierte** con
`index out of bounds`, así que el banco de pruebas distingue éxito de fracaso y el resultado de
arriba significa algo.

**Consecuencia doble:**

1. **Del registry:** cualquiera puede colgarle una respuesta al feedback de cualquiera. No es
   nuestro para arreglar, pero sí para no publicitar como si estuviera restringido.
2. **Nuestra, y es la que importa:** `POST /feedback/response` es **anónimo** y lo firmamos
   nosotros, así que el evento `ResponseAppended` registra al **facilitador** como `responder`.
   Es la misma forma del problema del revoke — un POST sin credenciales nos hace firmar — sólo
   que en vez de destruir reputación, ata nuestra identidad on-chain a contenido de un tercero y
   gasta nuestro gas. Lo acota el rate limit (~5/min) y el writer lease, nada más.

**No lo cerré por mi cuenta**: poner auth ahí rompe integraciones y no estaba en el reparto
acordado. Y la vía de autoría real tampoco existe todavía — el `FeedbackDelegate` sólo admite
dos selectores (`giveFeedback`, `revokeFeedback`), así que relayar un `appendResponse` con el
rater como autor **necesita un cambio en el contrato de Execution Market**.

---

## 5. La fase 2 del gate no se puede juzgar todavía: no hay tráfico

El rollout en dos tiempos existe para **medir antes de romper**. La medición, sobre la ventana
completa de retención de logs (6 días):

| línea de log | eventos |
|---|---|
| `Processing ERC-8004 feedback` | **3** |
| veredictos del gate (`feedback proof …`) | **0** |
| `Revoking ERC-8004 feedback` | 1 |
| respuestas anexadas | 0 |

Los 3 feedbacks son anteriores al deploy del gate. O sea: **el gate no ha visto ni una
submission desde que está vivo.** Prender `ERC8004_REQUIRE_PROOF=true` hoy sería enforcear sin
haber medido nada — exactamente lo que la fase 1 existe para evitar.

Y hay un dato de fondo: **~0,5 feedbacks por día**. Los 1384 mal atribuidos son historia
acumulada, no caudal actual.

---

## 6. Qué falta, por dueño

### Nuestro (facilitador)

- **Prender la fase 2** (`ERC8004_REQUIRE_PROOF=true`): bloqueado por falta de tráfico, no por
  código. Es una env var de la task definition. Antes hay que ver veredictos reales en los logs.
- **Cerrar el camino viejo de SVM** (`ERC8004_ALLOW_FACILITATOR_AUTHORSHIP=false`): cuando los
  callers migren a `/feedback/solana/prepare` + `/submit`. Hoy está abierto y ruidoso en logs.
- **`POST /feedback/response`**: decidir qué hacer con §4. Las opciones son gate admin (como el
  revoke), exigir prueba de titularidad del agente, o esperar el cambio de contrato de EM.
  Lo que **sí** se corrigió ya, porque era nuestro y no rompe nada: dejamos de anunciarlo como
  "agent only" en `GET /feedback` y en el OpenAPI. Publicar una restricción que no existe es
  peor que no publicar nada.

### De Execution Market

- **Desplegar el `FeedbackDelegate` en las redes que faltan.** El spec está escrito y entregado:
  `docs/handoffs/2026-08-14-feedbackdelegate-deployment-spec.md`. Son **16 redes, no 18** (SKALE
  queda afuera por EVM anterior a Shanghai; Solana no necesita contrato). 7 tienen 7702 probado
  con tráfico tipo-4 real; las otras 9 llevan un ensayo de una transacción.
- **Ampliar el delegate a `appendResponse`**, si se quiere autoría real también ahí (§4).
- **Mandar el neto, no el bruto**, en el `ProofOfPayment` (§2).

### De Saul

- **Los 1384 feedbacks históricos con autoría nuestra.** Las dos sesiones coinciden en no
  tocarlos: limpiarlos con `revoke` sería estrenar exactamente el poder del que nos queremos
  deshacer. Falta la confirmación.
- **¿El revoke queda admin-only para siempre**, o se reabre cuando el rater pueda firmar el suyo
  vía delegate?
- **Autorizar el deploy de mainnet del delegate** del lado de EM.

---

## 7. Dónde está cada cosa

| qué | dónde |
|---|---|
| Plan acordado con EM | `docs/handoffs/2026-08-13-erc8004-autoria-reputacion-p0.md` |
| H2 / stipend 7702 | `docs/handoffs/2026-08-14-eip7702-stipend-h2.md` |
| Spec de despliegue del delegate | `docs/handoffs/2026-08-14-feedbackdelegate-deployment-spec.md` |
| Gate del proof | `src/erc8004/proof.rs` |
| Relay 7702 | `src/erc8004/relay.rs` (tabla de delegates por red) |
| Autoría SVM | `src/erc8004/solana.rs` (`accept_rater_signed_transaction`) |
| Cerrar la cadena on-chain | `scripts/verify_feedback_anchor.py` |
| Medición del stipend | `scripts/spike_eip7702_stipend.sh` |
