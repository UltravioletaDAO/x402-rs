---
date: 2026-08-24
tags:
  - type/handoff
  - domain/identity
  - domain/blockchain
  - priority/p0
status: proposed
origen: execution-market
destino: x402-rs
aliases:
  - El struct v4 ya está cerrado
  - Entrega pendiente detectada por c0der
related-files:
  - docs/handoffs/2026-08-24-respuesta-a-execution-market-v3-y-eip712.md
  - src/erc8004/relay.rs
---

# Lo que pidieron para desbloquearse ya está escrito. No había llegado acá.

> **Entrega hecha por:** c0der (PM del stack) — **no** por el equipo de Execution Market.
> **Contenido de origen:** `execution-market/docs/handoffs/2026-08-24-facilitador-struct-v4-cerrado.md`,
> commiteado allá en `cffd50f0` (2026-08-24 21:30).
> **Por qué existe esta nota:** medido el 2026-08-24, ese archivo vive sólo en el
> repo de Execution Market. Un `grep` de `relayGiveFeedback`, `RelayedGiveFeedback`
> y `struct v4` sobre todo `x402-rs` devuelve **un único archivo: la respuesta de
> ustedes**. La contestación existe hace horas y de este lado no está registrada.

## Por qué es urgente y no sólo "un doc que falta"

Su handoff `2026-08-24-respuesta-a-execution-market-v3-y-eip712.md` cierra con:

> *"Lo que necesitamos de ustedes antes de escribir código: el shape final del struct."*

Execution Market lo respondió el mismo día. Y el orden de despliegue acordado
—registrado en `execution-market/contracts/deployments/feedback-delegate.json`,
clave `source_ahead_of_deployments.deploy_order`— arranca justamente por ustedes:

```
1) El Facilitator confirma el struct.
2) Lo implementan detrás de un flag, sirviendo AMBOS digests.
3) Recién entonces EM despliega v4 y les entrega la tabla.
4) Ustedes apuntan el flag por red.
5) Se apaga el digest viejo.
```

**El paso 1 lleva horas desbloqueado sin que este lado lo sepa.**

Y tiene una ventana que se cierra sola. Del mismo archivo:

> *"Redesplegar es gratis SÓLO mientras no exista una autorización EIP-7702 viva,
> y esa ventana se cierra sola en el momento en que el primer rater firme desde
> el dashboard."*

O sea: esto no vence por calendario, vence por el primer usuario que califique.
Después de eso, cambiar el delegate deja de ser gratis.

## El struct final, textual

Copiado sin editar del handoff de origen, sección 2. Domain, sin `salt`:

```
name:              "FeedbackDelegate"
version:           "1"
chainId:           <chain>
verifyingContract: <la cuenta del rater>   // address(this) bajo 7702
```

Typehashes, tal como los hashea el contrato:

```
RelayedGiveFeedback(address registry,uint256 agentId,int128 value,uint8 valueDecimals,string tag1,string tag2,string endpoint,string feedbackURI,bytes32 feedbackHash,uint256 deadline,bytes32 nonce)

RelayedRevokeFeedback(address registry,uint256 agentId,uint64 feedbackIndex,uint256 deadline,bytes32 nonce)

RelayedAppendResponse(address registry,uint256 agentId,address clientAddress,uint64 feedbackIndex,string responseURI,bytes32 responseHash,uint256 deadline,bytes32 nonce)

RelayedCancelNonce(bytes32 nonce,uint256 deadline)
```

Entradas:

```solidity
relayGiveFeedback(GiveFeedback calldata f, bytes calldata signature)
relayRevokeFeedback(RevokeFeedback calldata r, bytes calldata signature)
relayAppendResponse(AppendResponse calldata a, bytes calldata signature)
cancelNonceWithSig(bytes32 nonce, uint256 deadline, bytes calldata signature)
```

Tres decisiones que EM tomó por encima de la propuesta de ustedes, y que
explícitamente dejó abiertas a discusión antes de que escriban código:

1. **`registry` va en los tres structs y se verifica** contra el inmutable, con
   `WrongRegistry(address)`. El argumento: la wallet lo muestra, y un campo
   mostrado pero no verificado se lee como una garantía que no existe. Si de este
   lado prefieren omitirlo, hay que sacarlo también del display.
2. **`cancelNonce` pasa a EIP-712** con typehash propio. Bajo EIP-191 la
   separación entre un cancel y un relay dependía de que los preimages
   difirieran por conteo de campos; ahora es estructural.
3. **Los `string` se hashean como manda EIP-712** (`keccak256(bytes(s))` dentro
   del `encodeData`). Lo aclaran porque es el error de implementación más común
   y produce un mismatch que no dice nada.

Para el cruce de digests hay tres getters —`giveFeedbackDigest`,
`revokeFeedbackDigest`, `appendResponseDigest`— más `domainSeparator()`. El mismo
método que usaron con `relayDigest()` en v3, que dio idéntico byte a byte.

## Los dos puntos suyos que EM ya cerró

- **`POST /feedback/revoke` SÍ está autenticado.** EM lo midió contra producción:
  sin credenciales devuelve `401`. Reconocen el error y explican su causa —
  arrastraron un pendiente de un handoff anterior a la lista siguiente sin
  volver a medirlo.
- **ERC-1271.** Confirmado que v3 responde `supportsInterface(0x1626ba7e) -> false`.
  v4 lo anuncia, con test sobre los cuatro interface ids. El probe `0x150b7a02`
  que ustedes eligieron era el correcto y puede quedarse: distingue v1 de v3/v4.

## Qué se espera de este lado

Confirmar o discutir el struct de arriba. Eso es todo lo que falta para el paso 1.
Mientras no pase, v4 está escrito y con 25 tests en verde pero **con cero
despliegues**, y las 9 direcciones vivas siguen siendo v3 con el agujero de
`valueDecimals` que ustedes mismos encontraron.

## Nota de procedimiento

Las dos puntas escriben sus handoffs en su propio repo y ninguna queda en el
destino. Funciona mientras alguien los cargue a mano; falla en silencio cuando
no. Este caso es el ejemplo: los dos lados esperando al otro, con la respuesta
escrita y commiteada desde las 21:30.

El patrón propuesto para evitarlo está en
`control-plane/04-patterns/handoff-entre-proyectos.md`: el handoff se escribe en
`<repo-destino>/docs/handoffs/`, y un `accepted` que no aparece en el backlog del
destino es una entrega perdida —una verificación que c0der puede correr sola.
