# X-TANDA-R425 — `/register` no acuña lo que no debe, retiro de identidades custodiadas, tope temporal de Ethereum y el drift gate de IAM

**Encargo:** c0der, 2026-09-26 17:00Z (tanda de x402-rs; puntos M5 y L4 del informe del incidente R425, decisión D4,
decisión 85 del dueño y O-37 de c0der).
**Base:** `origin/main` = `b206eac5` (re-medido al llegar). **Rama:** `0xultravioleta/c0-x-tanda-r425`.
**Versión:** `2.44.0` (`VERSION`; producción no se consultó, está prohibido en este encargo: el bump sale del
`VERSION` de `main`, 2.43.0).

## Estado

- **Hecho, los cuatro puntos**, con tests sin red ni credenciales y cada guarda con su mutación (§7: 44/44 en rojo), más lo que encontró la revisión de seguridad (§5). Lo que queda para c0der está en §8.
- **Falta (de c0der, después del deploy):** §8 "Para c0der". Nada de esto se hizo acá: ni deploy, ni plan, ni
  llamadas a producción, ni AWS.

## 1. M5 — `POST /register` (`src/handlers.rs::refuse_registration`, `src/erc8004/agent_uri.rs`)

Un único punto, antes del lock en vuelo, de cualquier lectura de cadena y de las dos familias (EVM y Solana) y los
dos caminos (sync y `Prefer: respond-async`). Seis rechazos, en este orden (sólo el último lee la cadena):

1. **`agentUri`**: sólo `https://` en un nombre DNS público, o `ipfs://<cid>`, hasta 2048 bytes. Cada regla tiene
   su `errorCode` (`agent_uri_missing | too_long | malformed | scheme | credentials | ip_literal | non_public_host |
   embedded_ip | tunnel`). La IP literal se detecta en todas las formas que un parser WHATWG lee como IP (decimal,
   hex, octal, percent-encoded, IPv6); la IP embebida, por cuatro octetos seguidos separados por `-` o `.` en
   cualquier dominio **y** por la lista de servicios de DNS comodín (sslip.io, nip.io, xip.io...); los túneles por
   su lista. Las listas viven en `config/erc8004_agent_uri_rules.json` (compilado en el binario y leído también por
   el script de auditoría). Espacios, caracteres de control y `\` se rechazan como `malformed`: el parser WHATWG
   elimina los primeros y corta el host en el `\`, mientras que el de Python lee el host después del `@`
   (`https://bueno\@198.51.100.7/` es `bueno` para uno y la IP para el otro), así que lo que se juzga dejaría de ser
   lo que lee quien sigue la URI. Un `ipfs://` sólo acepta el CID como autoridad (ni usuario ni puerto).
2. **`recipient` obligatorio** (`400 recipient_required`): sin él el facilitador se quedaba el NFT.
3. **El facilitador no puede ser el `recipient`** (`400 recipient_is_facilitator`), que sería quedárselo con otro
   nombre (y en EVM el atajo de idempotencia devolvía una identidad cualquiera de las custodiadas); tampoco la
   dirección cero (`400 recipient_invalid`).
4. **Screening del `recipient`** contra las mismas listas que `/verify` y `/settle` (OFAC + blacklist propia),
   vía un método nuevo del trait `Facilitator::screen_recipient` (default: no filtra; `FacilitatorLocal` usa el
   `ComplianceChecker`). Bloqueada → `403 recipient_blocked`, sin acuñar **ni reclamar**; listas ilegibles →
   `503 recipient_screening_unavailable`, `retryable`. Va antes de cualquier camino que entregue una
   identidad.
5. **En EVM, el `recipient` tiene que poder recibir el NFT** (`400 recipient_cannot_receive`): el mint cae en
   nuestra wallet y el `safeTransferFrom` viene después; un contrato sin `onERC721Received` lo hace revertir y la
   identidad **se queda con nosotros** (y cada reintento acuñaba otra). Se simula el hook con `eth_call` desde el
   registro, como lo va a llamar el transfer; una dirección sin código siempre recibe. Si no se puede leer el
   código: `503 recipient_check_unavailable`.
6. Además, dentro de `run_evm_registration`, un `balanceOf` del `recipient` que no se puede leer ya no se saltea
   (era fail-open: seguía y acuñaba): `503` sin acuñar, la misma regla que el escaneo de dueño de al lado.

Medido, para no sobrevender: el *reclaim* EVM (`register_jobs.rs:28-37`) es por registro en memoria, dura 24 h y
sólo existe para un alta **con** `recipient` cuyo transfer falló; la identidad custodiada del informe se acuñó sin
`recipient`, así que ese camino nunca la alcanzó.

Lo que ya construyen los clientes del stack (`https://execution.market/{workers,publishers,agents}/<address>`, con
`recipient` siempre; relevado en EM, KK, los dos SDK y meshrelay) sigue pasando y hay un test que lo fija.

## 2. L4 / D4 — retiro de identidades custodiadas

- `POST /erc8004/admin/retire-identity` `{network, agentId, dryRun?}` (`src/erc8004/retire.rs` + handler): apunta
  el `agentURI` de una identidad EVM que tiene un firmante del facilitador a
  `https://facilitator.ultravioletadao.xyz/erc8004/retired`. **Por el servicio**: detrás de `ERC8004_ADMIN_TOKEN`
  (404 si no está) y del writer lease, con `setAgentURI` enviado desde el firmante dueño por el mismo provider y su
  `PendingNonceManager`. Nunca a mano con la llave caliente, nunca un transfer a una dirección muerta.
  - Lee `ownerOf` y `tokenURI` antes de nada: ajena → `409 not_held_by_facilitator`, sin enviar; `dryRun` →
    `would_retire` con la URI actual y las reglas de §1 que rompe; ya retirada → `already_retired`, sin enviar
    (repetir es gratis); recibo que no vuelve a tiempo → `504 unconfirmed` con la tx (repetir es seguro).
  - La URI de retiro es fija (así "ya retirada" se decide comparando) y pasa las reglas de §1 (test).
- `GET /erc8004/retired`: el archivo de registro ERC-8004 al que apuntan (`active: false`). Test de que se sirve
  exactamente donde apunta la URI.
- `scripts/erc8004_custodied_identities.py` (**sólo lectura**: su cliente RPC rechaza cualquier método fuera de
  `eth_blockNumber/chainId/getLogs/call/getCode` antes de enviarlo): enumera los `Transfer` ERC-721 **hacia** la EOA
  del facilitador (un mint es un transfer desde 0x0), se queda con los que `ownerOf` todavía le atribuye, lee cada
  `tokenURI` y la juzga con las **mismas** reglas que `/register`. El corpus
  `tests/fixtures/erc8004_agent_uri_cases.json` (44 casos, IPs de documentación) lo leen los tests de Rust **y** de
  Python, así que las dos implementaciones no pueden divergir. Compara el conteo con `balanceOf`. Las URIs
  sospechosas salen desactivadas (`hxxp`, `[.]`) salvo `--raw`. Registro por red leído de `src/erc8004/mod.rs`.

## 3. Decisión 85 — tope diario de Ethereum

`terraform/environments/production/main.tf`: `ERC8004_DAILY_WRITE_CAP_ETHEREUM=300` en el task def, con el
comentario de cuándo vuelve: **al terminar el relleno de KK se borra la entrada y vuelve el built-in de 100**, en el
release siguiente. Lo lee `daily_cap::from_env` al arrancar (ya existía; `/config` lo publica).

## 4. O-37 — el drift gate da rojo por IAM que el CI no puede escribir

Paso nuevo del job `plan` de `ci.yaml`, "IAM changes the deploy cannot apply" (`if: '!cancelled()'`), que corre
`scripts/drift_gate_iam.py` sobre `terraform show -json` del plan **dirigido**. Para cada `aws_iam_*` que cambia,
deduce las llamadas IAM que necesita (`PutRolePolicy`, `CreatePolicyVersion`...) y las evalúa contra la política del
CI **tal como está viva** (el `aws_iam_policy.cicd_infra` refrescado en el `prior_state` del plan completo; el
declarado sólo si todavía no está en el state). Deny explícito o ningún Allow → rojo, con el comando exacto:

```sh
cd terraform/environments/production
terraform version        # tiene que decir Terraform v1.9.8
terraform init -input=false
terraform apply -input=false \
  -target=aws_iam_role_policy.secrets_access
```

"Ningún Allow" es lo mismo que "denegado" acá: la otra política del usuario de CI (`facilitator-cicd`, inline) no da
ninguna escritura IAM salvo `iam:PassRole` (`docs/CICD_SETUP.md`). Un tipo IAM no clasificado, un rol que no se
conoce hasta el apply o una política no encontrada también dan rojo. Nunca imprime la cuenta ni un ARN. Los tests
evalúan contra la política **declarada** en `cicd-iam-policy.tf` (parseada del `jsonencode`), así que si esa política
cambia, los tests se mueven con ella. La sección "Pending" del reporte existente aclara que las filas IAM se juzgan
en la sección nueva.

## 5. Revisión de seguridad (agente auditor, sobre `55615081`) y lo que se hizo

| Hallazgo | Severidad | Qué se hizo |
|---|---|---|
| El facilitador se sigue quedando identidades con un `recipient` que no puede recibir (`0x0`, un contrato sin `onERC721Received`): mint, transfer que revierte, y reintentos que acuñan más | ALTA | **Arreglado** (§1.3, §1.5), con tests contra el nodo JSON-RPC de prueba |
| `balanceOf` ilegible se saltea (fail-open → mint duplicado) | (dentro de la anterior) | **Arreglado** (§1.6) |
| `\` hace que el `url` crate y Python vean hosts distintos | MEDIA | **Arreglado** en Rust y en el espejo de Python, con casos en el corpus |
| `ipfs://user@cid` e `ipfs://cid:8080`: Rust aceptaba, Python no | BAJA | **Arreglado** (Rust rechaza), casos en el corpus |
| Faltaban `.test`, `.invalid`, `.example`, `.onion`, `.localdomain`, `.corp`, `ts.net`, `app.github.dev`, `cfargotunnel.com` | BAJA | **Agregados** a `config/erc8004_agent_uri_rules.json` |
| El drift gate avisa pero no bloquea el deploy (`deploy` no depende de `plan`) | INFO | Deliberado y documentado en el propio job: la deriva previa no puede frenar un release. El pedido de O-37 es el rojo en el PR, con el comando |

Lo que el auditor revisó y dio por bueno: la ruta de retiro sólo se alcanza detrás del token (404/401 antes de leer
el cuerpo), la URI que escribe es la constante, `controls_signer(ownerOf)` antes de enviar, `.from(owner)` seguro
con varios firmantes (el `PendingNonceManager` lleva nonces por dirección), el gate corre antes del split async, del
lock y de los dos *reclaim*, el screening falla cerrado, los logs escapan y cortan la URI, el script de auditoría no
puede enviar nada, y `drift_gate_iam.py` no imprime cuentas ni ARNs.

## 6. Riesgos

| # | Riesgo | Qué pasa | Mitigación |
|---|---|---|---|
| R1 | Un cliente que hoy llama sin `recipient` | `400 recipient_required` | Ningún cliente del stack lo hace (relevado en EM, KK, los dos SDK y meshrelay). La mitad EM de M5 va en execution-market. Los SDK py/ts documentan `recipient` como opcional y sus ejemplos lo omiten: actualizar su documentación. |
| R2 | La URI de retiro se escribe on-chain | Queda en la cadena | Es de nuestro dominio y la servimos; el endpoint sólo toca identidades nuestras y es idempotente. |
| R3 | El drift gate da rojo por deriva IAM previa | Un PR ajeno a IAM ve rojo si el plan dirigido ya carga una diferencia de `secrets_access` o `balances_lambda_secrets` | Es lo pedido: el deploy de ese merge fallaría igual. Se aplica a mano con el comando del reporte y se re-corre. |
| R4 | Falso positivo de URI | Una identidad legítima en `*.sslip.io` o un túnel no se registra gratis | Deliberado (M5). Tienen dominio propio o IPFS. |
| R5 | El tope de Ethereum a 300 | Hasta ~US$12 extra de gas en Ethereum, una vez (decisión 85) | Se revierte borrando una entrada. |

## 7. Verificación (pre-CI local; el CI puede no arrancar hoy)

Entorno de todos los comandos: `AWS_SHARED_CREDENTIALS_FILE=/dev/null AWS_CONFIG_FILE=/dev/null`, sin
`AWS_PROFILE`, `HTTPS_PROXY/HTTP_PROXY=http://127.0.0.1:9`, `NO_PROXY=127.0.0.1,localhost` (el nodo JSON-RPC de
prueba vive en loopback), `cargo --offline`.

Todo sobre `9623d53d`, el último commit de código de la rama (el commit siguiente sólo agrega este handoff y el JSON de mutaciones). Cada paso del job `test` de `ci.yaml`, más los nuevos:

| Paso | Comando | Resultado |
|---|---|---|
| Landing (offline) | `python3 scripts/verify_landing_canonical.py --offline` | OK |
| Frontend | `node --test tests/frontend-capabilities.test.cjs` | 19/19 |
| Monitor de saldos | `python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'` | 5/5 |
| **Nuevo:** auditoría de custodiadas | `python3 -m unittest discover -s tests/scripts -p 'test_erc8004_*.py'` | 12/12 |
| **Nuevo:** drift gate de IAM | `python3 -m unittest discover -s tests/scripts -p 'test_drift_gate_*.py'` | 16/16 |
| Build | `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl,hedera` | exit 0 |
| x402-rs | `cargo test --locked -p x402-rs --features <los mismos> -- --test-threads=1` | lib 1451, bin 1516, integración 66; **0 fallas** (2 min 27 s) |
| Crates | `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | 109; **0 fallas** |
| Filtro de `paths` | `python3 scripts/ci_paths_selftest.py` | exit 0 (los dos scripts nuevos disparan el CI) |
| `no-account-id.yml` + hook hex64 | réplica local de sus reglas en Python, con un control positivo por regla, sobre todo el árbol y el diff | 0 hallazgos en 1069 archivos |
| `terraform fmt -check` | `main.tf` | exit 0 (no se corrió `plan` ni `apply`: prohibido en este encargo) |

Nota: una corrida local **en paralelo** del subconjunto `payment_operator` dio un rojo en
`arc_tests::arc_v3_writes_require_operator_code` (estado global de `autoverify`); en serie, como corre el CI, pasa.
No depende de este cambio.

### Mutaciones

Corrida sobre `9623d53d` con `c0der/scripts/verificar_ronda.py` (worktree propio en LF, red cerrada): suite `rc=0` (149 s), **44/44 mutaciones en rojo**, árbol limpio al final, veredicto «todo como pide la ronda». En la primera corrida (sobre `55615081`) sobrevivieron dos, `M5-uri-cid-ipfs` y `L4-ruta-no-montada`, y la segunda ronda de tests las mata: casos `ipfs://` inválidos en el corpus, y el test de ruteo ahora exige llegar al handler, porque el fallback del router de revoke también pasa por la capa admin y daba 401 aunque la ruta no estuviera montada. El JSON de mutaciones queda en `docs/handoffs/X-TANDA-R425-mutaciones.json` para re-correrlo.

| # | Mutación | Archivo | Qué rompe | Test que la atrapa | Resultado |
|---|---|---|---|---|---|
| 1 | `M5-uri-esquema` | `src/erc8004/agent_uri.rs` | acepta cualquier esquema | `erc8004::agent_uri` | **ROJO** (rc=101, 22 s) |
| 2 | `M5-uri-espacios-y-control` | `src/erc8004/agent_uri.rs` | acepta espacios, control y `\` | `erc8004::agent_uri` | **ROJO** (rc=101, 21 s) |
| 3 | `M5-uri-ip-literal` | `src/erc8004/agent_uri.rs` | acepta un host IP | `erc8004::agent_uri` | **ROJO** (rc=101, 21 s) |
| 4 | `M5-uri-ip-embebida-patron` | `src/erc8004/agent_uri.rs` | sin el patrón de 4 octetos | `erc8004::agent_uri` | **ROJO** (rc=101, 21 s) |
| 5 | `M5-uri-dns-comodin-lista` | `src/erc8004/agent_uri.rs` | sin la lista de DNS comodín | `erc8004::agent_uri` | **ROJO** (rc=101, 21 s) |
| 6 | `M5-uri-sslip-fuera-del-config` | `config/erc8004_agent_uri_rules.json` | borra `sslip.io` del JSON de reglas | `erc8004::agent_uri` | **ROJO** (rc=101, 20 s) |
| 7 | `M5-uri-tuneles` | `src/erc8004/agent_uri.rs` | acepta túneles | `erc8004::agent_uri` | **ROJO** (rc=101, 20 s) |
| 8 | `M5-uri-credenciales` | `src/erc8004/agent_uri.rs` | acepta credenciales | `erc8004::agent_uri` | **ROJO** (rc=101, 17 s) |
| 9 | `M5-uri-host-no-publico` | `src/erc8004/agent_uri.rs` | acepta hosts no públicos | `erc8004::agent_uri` | **ROJO** (rc=101, 20 s) |
| 10 | `M5-uri-largo` | `src/erc8004/agent_uri.rs` | sin tope de largo | `erc8004::agent_uri` | **ROJO** (rc=101, 20 s) |
| 11 | `M5-uri-cid-ipfs` | `src/erc8004/agent_uri.rs` | acepta cualquier autoridad `ipfs://` | `erc8004::agent_uri` | **ROJO** (rc=101, 21 s) |
| 12 | `M5-gate-sin-chequeo-de-uri` | `src/handlers.rs` | el gate no mira la URI | `erc8004_register_gate` | **ROJO** (rc=101, 24 s) |
| 13 | `M5-gate-sin-recipient-pasa` | `src/handlers.rs` | sin `recipient` sigue al mint | `erc8004_register_gate` | **ROJO** (rc=101, 25 s) |
| 14 | `M5-gate-facilitador-como-recipient` | `src/handlers.rs` | acepta al facilitador como recipient | `erc8004_register_gate` | **ROJO** (rc=101, 25 s) |
| 15 | `M5-gate-bloqueada-pasa` | `src/handlers.rs` | una wallet bloqueada pasa | `erc8004_register_gate` | **ROJO** (rc=101, 21 s) |
| 16 | `M5-gate-listas-ilegibles-pasan` | `src/handlers.rs` | listas ilegibles = libre | `erc8004_register_gate` | **ROJO** (rc=101, 25 s) |
| 17 | `M5-gate-no-montado` | `src/handlers.rs` | el gate no se llama | `erc8004_register_gate` | **ROJO** (rc=101, 29 s) |
| 18 | `M5-screening-real-no-bloquea` | `src/facilitator_local.rs` | `FacilitatorLocal` nunca bloquea | `screen_recipient_tests` | **ROJO** (rc=101, 27 s) |
| 19 | `L4-retiro-identidad-ajena` | `src/erc8004/retire.rs` | retira identidades ajenas | `erc8004::retire` | **ROJO** (rc=101, 24 s) |
| 20 | `L4-retiro-dry-run-envia` | `src/erc8004/retire.rs` | el dry run envía | `erc8004::retire` | **ROJO** (rc=101, 22 s) |
| 21 | `L4-retiro-no-idempotente` | `src/erc8004/retire.rs` | reenvía aunque ya esté retirada | `erc8004::retire` | **ROJO** (rc=101, 22 s) |
| 22 | `L4-retiro-desde-otro-firmante` | `src/erc8004/retire.rs` | envía desde el firmante por defecto, no el dueño | `erc8004::retire` | **ROJO** (rc=101, 24 s) |
| 23 | `L4-retiro-revert-como-exito` | `src/erc8004/retire.rs` | un revert cuenta como retirada | `erc8004::retire` | **ROJO** (rc=101, 23 s) |
| 24 | `L4-ruta-sin-token-admin` | `src/handlers.rs` | la ruta sin la capa admin | `erc8004_admin_gate_tests` | **ROJO** (rc=101, 28 s) |
| 25 | `L4-ruta-no-montada` | `src/handlers.rs` | la ruta no se monta | `erc8004_admin_gate_tests` | **ROJO** (rc=101, 17 s) |
| 26 | `L4-documento-de-retiro-no-servido` | `src/handlers.rs` | `GET /erc8004/retired` no se sirve | `erc8004_admin_gate_tests` | **ROJO** (rc=101, 27 s) |
| 27 | `L4-script-escribe` | `scripts/erc8004_custodied_identities.py` | el cliente RPC deja pasar métodos de escritura | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 28 | `L4-script-cuenta-las-que-salieron` | `scripts/erc8004_custodied_identities.py` | lista identidades que ya salieron de la wallet | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 29 | `L4-script-reglas-divergen` | `scripts/erc8004_custodied_identities.py` | el espejo Python pierde la regla de IP embebida | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 30 | `L4-script-sin-desactivar` | `scripts/erc8004_custodied_identities.py` | imprime las URIs sospechosas sin desactivar | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 31 | `L4-script-salta-rangos-rechazados` | `scripts/erc8004_custodied_identities.py` | saltea el rango que el nodo rechaza | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 32 | `O37-deny-ignorado` | `scripts/drift_gate_iam.py` | ignora los Deny | `test_drift_gate_*.py` | **ROJO** (rc=1, 0 s) |
| 33 | `O37-nunca-rojo` | `scripts/drift_gate_iam.py` | nunca sale rojo | `test_drift_gate_*.py` | **ROJO** (rc=1, 0 s) |
| 34 | `O37-tipo-iam-desconocido-pasa` | `scripts/drift_gate_iam.py` | un tipo IAM no clasificado pasa | `test_drift_gate_*.py` | **ROJO** (rc=1, 0 s) |
| 35 | `O37-sin-politica-pasa` | `scripts/drift_gate_iam.py` | sin la política del CI, todo pasa | `test_drift_gate_*.py` | **ROJO** (rc=1, 0 s) |
| 36 | `O37-imprime-la-cuenta` | `scripts/drift_gate_iam.py` | imprime el ID de la cuenta | `test_drift_gate_*.py` | **ROJO** (rc=1, 0 s) |
| 37 | `O37-paso-se-salta-tras-rojo` | `.github/workflows/ci.yaml` | el paso sin `if: '!cancelled()'` | `test_drift_gate_*.py` | **ROJO** (rc=1, 0 s) |
| 38 | `M5-uri-contrabarra` | `src/erc8004/agent_uri.rs` | acepta `\` | `erc8004::agent_uri` | **ROJO** (rc=101, 28 s) |
| 39 | `M5-uri-ipfs-con-usuario-o-puerto` | `src/erc8004/agent_uri.rs` | acepta `ipfs://user@cid` y `:puerto` | `erc8004::agent_uri` | **ROJO** (rc=101, 21 s) |
| 40 | `M5-gate-direccion-cero` | `src/handlers.rs` | acepta la dirección cero | `erc8004_register_gate` | **ROJO** (rc=101, 24 s) |
| 41 | `M5-gate-sin-chequeo-de-receptor` | `src/handlers.rs` | no mira si el recipient puede recibir | `erc8004_register_gate` | **ROJO** (rc=101, 25 s) |
| 42 | `M5-gate-contrato-sin-hook-pasa` | `src/handlers.rs` | un contrato sin `onERC721Received` pasa | `erc8004_register_gate` | **ROJO** (rc=101, 25 s) |
| 43 | `M5-gate-codigo-ilegible-pasa` | `src/handlers.rs` | código ilegible = puede recibir | `erc8004_register_gate` | **ROJO** (rc=101, 21 s) |
| 44 | `M5-balance-ilegible-acuna` | `src/handlers.rs` | `balanceOf` ilegible sigue y acuña | `erc8004_register_gate` | **ROJO** (rc=101, 23 s) |

### Ronda 2 (refutador REF-X-TANDA-R425: MERGEABLE CON RONDA, sin cambio de comportamiento)

Tests nuevos, cada uno contra una mutación del refutador que antes sobrevivía:
- `/register` con `Prefer: respond-async` y una URI rechazada: 400 con su código, nunca 202, y ningún job en vuelo.
- Con un `SolanaProvider` real detrás (RPC en un listener local que cuenta conexiones): una URI rechazada da 400 y
  `mint.status = not_minted`, y el fee payer como `recipient` da `recipient_is_facilitator`. Cero conexiones al RPC.
- Un receptor que contesta `onERC721Received` con otro `bytes4`: `recipient_cannot_receive`.
- El screening: una decisión `Review` bloquea igual que `Block`.
- El corpus suma un túnel con punto final y una contraseña sin usuario (Rust y Python dan lo mismo).
- La ruta de retiro, en una tarea **sin** writer lease: 404 sin token y 401 con token, nunca 503.
- El drift gate: la política viva gana sobre la declarada, un Allow con `Condition` no concede, y un ARN sin cuenta
  no concede.

Texto: se sacaron del CHANGELOG, de este handoff, del comentario de `refuse_registration` y del texto de `/erc8004`
la enumeración de los caminos que entregan una identidad y la frase "sin recipient, el NFT se queda con el
facilitador" (el `recipient` es obligatorio desde 2.44.0).

Mutaciones de esta ronda: las 44 de arriba más 23 del refutador. c0der registra su corrida sobre este commit.

## 8. Para c0der

1. **Merge y deploy** (un CI y un deploy): el plan dirigido de este PR sólo cambia `aws_ecs_task_definition` (una
   variable de entorno nueva); no toca IAM, así que el paso nuevo del drift gate debería salir limpio salvo deriva
   IAM previa (R3).
2. **Versión:** `/version` = `2.44.0`.
3. **Canario M5** (ninguno acuña nada; todos se rechazan antes de la cadena):
   - `agentUri: "http://198-51-100-7.sslip.io/a.json"` con un `recipient` → `400 agent_uri_scheme`.
   - `agentUri: "https://198-51-100-7.sslip.io/a.json"` → `400 agent_uri_embedded_ip`.
   - sin `recipient` → `400 recipient_required`; `recipient` = `0x000…0` → `400 recipient_invalid`.
   - `recipient` = un contrato sin `onERC721Received` (el propio IdentityRegistry de Base,
     `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432`) → `400 recipient_cannot_receive` (lee la cadena, no acuña).
   - El canario legítimo es el alta normal de EM (`https://execution.market/workers/<wallet>` con `recipient`).
4. **Retirar la identidad custodiada del informe (#95531 en Base)**, primero en seco:
   ```sh
   curl -s -X POST https://facilitator.ultravioletadao.xyz/erc8004/admin/retire-identity \
     -H "Authorization: Bearer $ERC8004_ADMIN_TOKEN" -H 'Content-Type: application/json' \
     -d '{"network":"base","agentId":"95531","dryRun":true}'
   ```
   Esperado: `status: would_retire`, `owner` = la EOA del facilitador, `previousUriViolations` =
   `["agent_uri_scheme","agent_uri_embedded_ip"]`. Después, lo mismo sin `dryRun` → `status: retired` y
   `transaction`. Prueba de hecho: `tokenURI(95531)` por `eth_call` = `https://facilitator.ultravioletadao.xyz/erc8004/retired`;
   repetir la llamada → `already_retired`. `ERC8004_ADMIN_TOKEN` ya está mapeado en el task def (`secrets.tf`).
5. **Auditoría de las custodiadas** (sólo lectura, con el RPC de Base que prefieras):
   ```sh
   python3 scripts/erc8004_custodied_identities.py --rpc "$RPC_URL_BASE" > custodiadas.md
   ```
   Si el nodo no responde `eth_getCode` histórico, pasar `--from-block` (el primer bloque del registro). El resumen
   final compara `found` con `balanceOf` (174 medido en el informe); si no coinciden, ampliar el rango antes de
   confiar en la tabla. Cada fila marcada se retira con el paso 4. Exit 2 = hay marcadas.
6. **Cuando termine el relleno de KK:** borrar `ERC8004_DAILY_WRITE_CAP_ETHEREUM` de `main.tf` (vuelve a 100) en la
   tanda siguiente.
7. **Mitad EM de M5** (fuera de este repo): la validación equivalente del lado de execution-market (este repo expone
   la lista en `config/erc8004_agent_uri_rules.json` y el corpus en `tests/fixtures/erc8004_agent_uri_cases.json`).
