---
date: 2026-09-23
tags:
  - type/handoff
  - domain/arc
  - domain/erc8004
status: active
---

# Relé de feedback ERC-8004 en Arc mainnet (2.38.0)

**Base:** `35744d12` (2.37.1, en producción). **Rama:** `c0der/rele-arc`. **Publica
`2.38.0`** (minor: una capacidad nueva en una red, como 2.37.0).

R3 del plan Arc. Desde 2.37.0 Arc sirve ERC-8004, pero el camino donde el calificador
firma y el facilitador sólo paga el gas (`POST /feedback/evm/prepare` + `/submit`)
contestaba `400 relayed feedback is not available on arc`, porque no había
`FeedbackDelegate` en Arc. Execution Market lo desplegó (R2, EM #307) y este cambio lo
conecta. Es configuración: no se despliega nada on-chain.

## Qué cambia

- `src/erc8004/relay.rs`, `delegate_address()` (el único mapa red → delegate;
  `relay_v4.rs` arma dominio y calldata por `chain_id`):
  - `Network::Arc` → `0x955Cc9fB9aB95FC0821ae74197D273dde5dA84f1` (v4).
  - `Network::BaseSepolia` → `0x9551263b9B83b1A737D55fd5e67Fb6D60e4eF787` (v4), en
    lugar de `0x1AaEA468…5b45` (v3). Es el drift que midió el refutador de EM #307, y
    se confirma abajo.
  - `arc-testnet` sigue sin delegate (no hubo despliegue en testnet).
  - El comentario de la tabla mostraba las direcciones v3 aunque el código servía v4
    desde 2026-08-25. Ahora lista las diez v4 con lo que se midió hoy.
- Tests del relé:
  - `the_mainnet_delegates_are_the_verified_ones` suma `arc`;
    `the_base_sepolia_delegate_is_the_verified_one` fija la v4.
  - Nuevo `arc_is_relayed_and_arc_testnet_is_not`: `arc` tiene la dirección
    verificada y su `get_contracts` es el registro de mainnet (con el que se compara el
    delegate en cada request). `arc-testnet` sirve ERC-8004 pero no tiene delegate.
  - `the_chains_without_a_delegate_claim_none` pierde `arc` y conserva `arc-testnet`.
  - `the_superseded_addresses_are_gone` suma las tres direcciones viejas de Base
    Sepolia. Una de ellas (`0x955C…84f1`) es la v4 viva de Arc: mismo deployer, mismo
    nonce. `an_address_alone_does_not_identify_a_version` lo deja escrito.
  - Nuevo `the_erc8004_page_names_exactly_the_networks_with_a_delegate`: la frase de
    `/erc8004` (el markup y los diccionarios EN y ES) nombra exactamente las redes que
    tienen delegate.
- `src/openapi.rs`: la lista de disponibilidad de `/feedback/evm/prepare` suma `arc`.
  El ejemplo de `/feedback/evm/submit` autorizaba `0x3A68…3768`, una dirección de Base
  Sepolia que el facilitador rechaza con `relay_authorization_wrong_delegate`. Ahora
  usa la v4. Nuevo test `the_relay_prose_names_exactly_the_networks_with_a_delegate`
  ata la lista y el ejemplo a `feedback_delegate()`.
- Otras listas que estaban mal:
  - `static/erc8004.html` (markup, `en` y `es`) decía «hoy únicamente
    `base-sepolia`», falso desde que hay delegates en mainnet. Ahora nombra las diez
    redes.
  - `docs/networks/arc.md` decía que Arc no tiene relé.
  - `.env.example` y `CLAUDE.md` decían «sólo Base Sepolia», y `CLAUDE.md` todavía
    daba la dirección de 2026-08-14.
  - **Arc no aparece en la portada**: ni en un título ni en la vista previa. Sólo se
    nombra dentro de las listas de redes.
- `VERSION` 2.37.1 → 2.38.0 y la entrada en `CHANGELOG.md` (el de la raíz).

**No cambia:** el código del relé (verificación, sonda de versión, digests), `upto`,
escrow, `/supported`, el tope diario (Arc sigue en 100 escrituras por día) ni
`POST /feedback`. Donde hay delegate, `POST /feedback` sigue funcionando y
sólo agrega un `warn!` de ruta deprecada, que ahora también sale en Arc.

## Lo medido (sólo lectura, 2026-09-23 04:4xZ)

La fuente de verdad es `contracts/deployments/feedback-delegate.json` en
execution-market, `origin/main` `b035d30c` (#307). De ahí salen las filas `arc` y
`base-sepolia`. Cada dirección se leyó en **dos RPC**:

| red | delegate | bytes | `VERSION()` | `REPUTATION_REGISTRY()` | `supportsInterface(0x378a0c90)` |
|---|---|---|---|---|---|
| arc | `0x955Cc9fB…84f1` | 5857 | 4 | `0x8004BAa1…9b63` (mainnet) | true |
| base-sepolia | `0x9551263b…F787` | 5857 | 4 | `0x8004B663…8713` (testnet) | true |
| base-sepolia (lo que servíamos) | `0x1AaEA468…5b45` | 3216 | **revierte** | `0x8004B663…8713` | false (v3: sólo `0x150b7a02`) |

- Los RPC fueron `rpc.mainnet.arc.io` y `arc-mainnet.drpc.org` (chainId 5042) para
  Arc, y `sepolia.base.org` y `base-sepolia-rpc.publicnode.com` (84532) para Base
  Sepolia. El registro de reputación tiene código en las dos redes (130 B).
- Las ocho v4 de mainnet también se releyeron, con dos RPC cada una. Las diez tienen
  el mismo código ejecutable (sha256 `66aef995e2b7241b…`) una vez neutralizadas las 5
  ocurrencias del registro y sacada la metadata CBOR. En las diez el registro
  aparece 5 veces.
- Las otras dos direcciones viejas de Base Sepolia (`0x3A68…3768`, `0x955C…84f1`)
  son pre-v3. `supportsInterface` revierte en las dos, así que la sonda de versión las
  rechazaría.

Producción (2.37.1) antes del release, con el mismo cuerpo que usa la sonda de cierre:
`arc` → **400** `relayed feedback is not available on arc: no FeedbackDelegate is
deployed there yet`; `base-sepolia` → 200 con el delegate `0x1aae…5b45`,
`signingPayload` y **sin** `typedData` (sirve v3).

## Efecto sobre quien ya usa Base Sepolia

Desde el release, `prepare` en Base Sepolia contesta como en las ocho mainnets: trae
`typedData` (EIP-712) y ya no trae `signingPayload`. Un cliente que en testnet sólo
sabía firmar el digest v3 va a recibir el mismo formato que ya recibe en mainnet. Un
calificador que tenga la cuenta delegada a la v3 aparece con `delegated: false` y
firma una autorización nueva (estado `Supersedable`, ya cubierto por tests). Además,
el par de respuesta (`/feedback/response/evm/*`, sólo v4) pasa a estar disponible en
Base Sepolia.

## Verificación (checkout con LF)

Los pasos que `scripts/preci.py --base origin/main` lista para este diff (dispara
`ci.yaml` y `no-account-id.yml`), corridos en local:

| Paso de CI | Resultado |
|---|---|
| `python3 scripts/verify_landing_canonical.py --offline` | `[OK]` |
| `node --test tests/frontend-capabilities.test.cjs` | 8/8 pass |
| `python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'` | 5 OK |
| `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl,hedera` | exit 0 |
| `cargo test --locked -p x402-rs --features <las mismas> -- --test-threads=1` | exit 0, **2562 passed, 0 failed**, 23 ignored (11 suites). Son +5 sobre las 2557 de #95: los 2 tests nuevos del relé corren en la lib y en el binario, y el de la OpenAPI sólo en el binario |
| `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | exit 0, 109 passed, 0 failed, 9 ignored |
| `no-account-id.yml` (sus tres expresiones, en Python sobre los archivos del diff) | sin coincidencias; ninguna línea nueva con `0x` + 64 hex |

`rustfmt --check`: `relay.rs` limpio. En `openapi.rs` el único diff es preexistente,
en un test de Hedera que no se tocó; `main` ya lo trae.

**Mutaciones** (cada una aplicada sobre la rama y revertida):

| Mutación | Falla |
|---|---|
| sacar `arc` de la lista de `/docs` | `the_relay_prose_names_exactly_the_networks_with_a_delegate`: `` `arc`: relayed feedback served = true, named in /docs = false `` |
| sacar `arc` del diccionario `es` de `/erc8004` | `the_erc8004_page_names_exactly_the_networks_with_a_delegate` (cita la frase en castellano) |
| Base Sepolia de vuelta a la v3 | `the_base_sepolia_delegate_is_the_verified_one` y `the_superseded_addresses_are_gone` (`base-sepolia is back on a superseded delegate`) |
| el ejemplo de `/submit` de vuelta a `0x3A68…` | `the /submit example authorises a delegate base-sepolia does not serve` |
| sacar la entrada `Network::Arc` | `arc_is_relayed_and_arc_testnet_is_not`, `the_mainnet_delegates_are_the_verified_ones` (`arc lost its FeedbackDelegate`), `an_address_alone_does_not_identify_a_version` y el test de `/erc8004` |

**Sonda con el binario de esta rama**, en local (2026-09-23 05:0xZ). Corrió en
127.0.0.1 con llaves desechables sin fondos, sólo `RPC_URL_ARC`, `RPC_URL_ARC_TESTNET`
y `RPC_URL_BASE_SEPOLIA`, `ENABLE_WRITER_LEASE=false` y `config/blacklist.json`
copiado del `.example`, que se borró después. El cuerpo es el de la sonda de cierre
de abajo:

| red | respuesta |
|---|---|
| `arc` | **200**: `delegate` `0x955c…84f1`, `typedData` (`RelayedGiveFeedback`; dominio `FeedbackDelegate`/`1`, `chainId` 5042, `verifyingContract` = el calificador), `delegated: false`, `accountNonce: 0` |
| `base-sepolia` | **200**: `delegate` `0x9551…f787`, `typedData` (chainId 84532), sin `signingPayload` |
| `arc-testnet` | **400** `relayed feedback is not available on arc-testnet: no FeedbackDelegate is deployed there yet` |

## Suposiciones (reversibles)

1. **2.38.0 y no 2.37.2**: abre una capacidad en una red, igual que 2.37.0 abrió
   ERC-8004 en Arc. Si otro PR sale antes con 2.38.0, este se sube a la siguiente.
2. **El drift de Base Sepolia va en este PR** (el encargo lo pedía si se confirmaba, y
   se confirmó on-chain). Para aislarlo habría que revertir la entrada `BaseSepolia`,
   el test que la fija, las tres filas nuevas de `the_superseded_addresses_are_gone` y
   la dirección del ejemplo de `/submit`. La entrada de Arc no depende de ese cambio.
3. **Las listas sólo nombran a Arc**, no la destacan. La portada (`index.html`) no se
   tocó.

## Para c0der (después del release)

La sonda de cierre. **No escribe nada on-chain**: `prepare` sólo lee
(`eth_getCode` del delegate, del registro y del calificador; `eth_call` a
`REPUTATION_REGISTRY()` y a `supportsInterface`, y `eth_getTransactionCount`). No
firma, no envía ninguna transacción y no descuenta del tope diario, que sólo cuenta
los `submit`. Gasta un token del presupuesto por IP de las escrituras ERC-8004. El
`rater` es `0x…dEaD`, que no tiene código en ninguna de las tres redes (medido hoy).

```bash
F=https://facilitator.ultravioletadao.xyz
curl -s $F/version                                   # {"version":"2.38.0"}
probe(){ curl -s -X POST $F/feedback/evm/prepare -H 'content-type: application/json' \
  -d "{\"x402Version\":1,\"network\":\"$1\",\"feedback\":{\"agentId\":1,\"value\":87,\"valueDecimals\":0,\"tag1\":\"probe\",\"rater\":\"0x000000000000000000000000000000000000dEaD\"}}" \
  -w '\nHTTP %{http_code}\n'; }
probe arc           # antes: HTTP 400 "relayed feedback is not available on arc ..."
                    # después: HTTP 200, "delegate":"0x955cc9fb9ab95fc0821ae74197d273dde5da84f1",
                    #   "typedData":{...}, sin "signingPayload", "chainId":5042, "delegated":false
probe base-sepolia  # después: "delegate":"0x9551263b9b83b1a737d55fd5e67fb6d60e4ef787" y "typedData"
                    #   (hoy: 0x1aae...5b45 con "signingPayload")
probe arc-testnet   # sigue HTTP 400 "relayed feedback is not available on arc-testnet ..."
```

Si `arc` contesta **503** con `relay_delegate_not_deployed`,
`relay_delegate_wrong_registry` o `relay_delegate_superseded_version`, el RPC que usa
producción para Arc no ve lo mismo que los dos que se midieron. Con
`relay_rpc_unavailable`, el RPC no contestó. En los dos casos no es la tabla.

## Lo que sigue en el plan (no es de este PR)

- **R4** (SDKs): `arc` en `RELAYED_FEEDBACK_NETWORKS`, sólo después de este release.
- **R4-KK**: la allowlist de KK tiene que leer la dirección de **este** archivo
  (`delegate_address()` en `src/erc8004/relay.rs`) y verificarla on-chain, no copiarla
  de un informe.
