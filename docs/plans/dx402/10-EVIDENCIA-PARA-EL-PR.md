> **Actualización 2026-09-04 (posterior a este documento):** las 5 filas de Solana marcadas `verified: true` por el código anterior a v1.82.0 fueron pasadas a `verified: false` (`notVerifiedReason: pre_gate_self_asserted_2026_08_18`). El corpus certificado es ahora **100 % EVM**; re-conteo tras la corrección: **119 verificados en 7 redes**. Los números de abajo son los medidos antes de esa corrección y se conservan tal cual; el §5 reproduce el estado actual.

# DX402 — Evidencia reproducible para el PR upstream

**Snapshot:** 2026-09-04 03:13Z. **Facilitador en producción:** `2.10.0` (`curl -s https://facilitator.ultravioletadao.xyz/version`).
**Fuente:** un `scan` completo de la tabla `facilitator_dx402_evidence` (us-east-2, 827 ítems, sin `LastEvaluatedKey`) más el API público. Cada número de este documento sale de un comando que se muestra; ninguno viene de un reporte ajeno.

Se excluye la dirección de test `0x1111111111111111111111111111111111111111` (6 filas la tocan como payer o payee, 1 de ellas `verified`). Todo lo que sigue está medido sin ellas.

---

## 1. Corpus

```bash
aws dynamodb scan --table-name facilitator_dx402_evidence --region us-east-2 --output json > scan.json
python3 corpus.py   # sección 5
```

| Métrica | Valor |
|---|---|
| Anclajes en la tabla | **827** (821 sin la dirección de test) |
| `verified: true` | **121** — 116 en cadenas EVM, 5 en Solana (ver §4: las de Solana **no** están verificadas contra la cadena) |
| `signed: true` | **111** (110 de ellas además `verified`) |
| Redes con `verified` | **8**: avalanche 49, arbitrum 37, optimism 9, base 7, monad 7, ethereum 6, solana 5, polygon 1 |
| Compradores distintos (entre `verified`) | **30** (26 contando sólo EVM) |
| Vendedores distintos (entre `verified`) | **28** (24 contando sólo EVM) |
| Primer / último `anchoredAt` (`verified`) | 2026-08-18T19:22:17Z / 2026-09-03T23:24:58Z |
| Primer / último `anchoredAt` (todos) | 2026-08-17T21:20:05Z / 2026-09-03T23:24:58Z |
| `keyAlg` entre `verified` | ECIES-secp256k1 116, ECIES-X25519 5 |
| `mode` / `retention` / backend entre `verified` | `direct` / `90d` en todos; ipfs 109, s3 12 |

Dos notas sobre la tabla misma:

- `/dx402/stats` reporta `anchored: 730` contra 827 filas: el contador es un piso por diseño (el propio endpoint lo dice: *"records whose index write failed are not counted"*). La tabla manda.
- 8 filas (2026-08-17/18) no tienen el atributo `verified` a nivel de ítem; son anteriores al esquema actual y cuentan como no verificadas. En las 819 restantes el BOOL de arriba coincide con el `verified` dentro de `record` (0 discrepancias).

## 2. Muestra reproducible

Diez `paymentId` con `verified: true`, repartidos por red y con vendedores distintos (polygon sólo tiene uno verificado y comparte payee con el de monad). Para cada uno:

```bash
curl -s https://facilitator.ultravioletadao.xyz/dx402/evidence/<paymentId> | jq '{verified,signed,receiptSigner}'
```

El API devuelve `verified`, `signed`, `receiptSigner` y la firma en `receipt`; la red y el `txHash` viven en `/dx402/receipt/<paymentId>` (`.receipt.network`, `.receipt.txHash`). Los 10 respondieron `receiptSigner: 0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF`.

| # | Red | `paymentId` | `verified` | `signed` | `txHash` |
|---|---|---|---|---|---|
| 1 | avalanche | `0x7e7ca6a10e0d2a835fd0d8045d0a95a6ee5f5dd29b39696e69c3c21c7a3387ec` | true | true | [`0x71799e37…22ced4`](https://snowtrace.io/tx/0x71799e37a07e5b8b7edf96f6c2c3b62455c4dad859c1885ef995fdd95622ced4) |
| 2 | arbitrum | `0x7ecc7e392df64e78a7aa07d7934ee75e6f08ca29300d86ba1382c576aff11111` | true | true | [`0xf9ba043d…e159e6`](https://arbiscan.io/tx/0xf9ba043d55a2871e4aeb54a4d4854bc92f8200f3e519a4feb8264a8f52e159e6) |
| 3 | optimism | `0x7611a8d13a5948cf3ebb3b1daf69125c7fbf6a71d9a1e610bf7aa5e9da816d10` | true | true | [`0x2a03c6a6…97bc30`](https://optimistic.etherscan.io/tx/0x2a03c6a64cef6a29358b548bc79470aa70062b549387fa6e4fbda4c61997bc30) |
| 4 | base | `0x96d9a8fbe441d2fd8abd3628d14644582d8f3b926f2a223edb0a5b75e8f9761f` | true | true | [`0x9aa235ef…a0c4cf`](https://basescan.org/tx/0x9aa235ef1ffeacf1f8a2731d208a8a959c452f90be1c84262385d71022a0c4cf) |
| 5 | monad | `0x1b5bc20fa48814cab84395c875a499f18d87038cead861f1e85e30bbfae9fda0` | true | true | `0x36afe04e…9d07ec8` — explorer: verificar por RPC (abajo) |
| 6 | ethereum | `0xe302ad23f94a2fd28973e64785b38bfae2d5b9e019c87d4be91ccbe98e29b0b4` | true | true | [`0xbd7b8fd1…d186d5`](https://etherscan.io/tx/0xbd7b8fd12f97754a032ccf95d79175403db2366a0c2f0ecb993e966584d186d5) |
| 7 | solana | `0xe386f89a4d382746034400e385f19ac6d66ea1db2f6d8be4c569e5d7eeb0626b` | true | **false** | `KKFIRMADEMOc559568b7f7d460c` — **no es una firma de Solana**; ver §4 |
| 8 | polygon | `0xbf805733abd51e074789ff6aacd3d5e8fdff7877808b430b2f78c8f5e6d2deec` | true | true | [`0xcc026f6c…4e768c`](https://polygonscan.com/tx/0xcc026f6c4c4fe0818c610d7c0cd76a2a7e9b1fb6bd077ec28d835e5c704e768c) |
| 9 | avalanche | `0xb80ec7227a9f1afa8aa1502d8817c6a0c52dc6d1b229c39e00f696fbc17a4aef` | true | true | [`0x2dc7f4ae…7e5f228`](https://snowtrace.io/tx/0x2dc7f4aea047ac074cc9d6fcf843066dda3a53a509e91bfb07c143cbe7e5f228) |
| 10 | arbitrum | `0xab6f4f98b4ad8c6eb33e83a7099bcdaa51ecd809cad69e11a275887b114cede1` | true | true | [`0x05d21828…626f1c7`](https://arbiscan.io/tx/0x05d218287eaeb53815e3f6c70989692ccf0b28c241f21c9eec4945bf1626f1c7) |

Dos de las transacciones se confirmaron contra la cadena además del explorer (`monadexplorer.com` redirige a `monadvision.com`, que responde 403 a `curl`):

```bash
$ cast receipt 0x9aa235ef1ffeacf1f8a2731d208a8a959c452f90be1c84262385d71022a0c4cf --rpc-url https://mainnet.base.org | grep -E '^(status|blockNumber|from)'
blockNumber          50839205
from                 0x103040545AC5031A11E8C03dd11324C7333a13C7   # el EOA del facilitador: settle nuestro
status               1 (success)

$ cast receipt 0x36afe04eba6bd1e17215c21e0b2307b96c04934d088418032100f755b9d07ec8 --rpc-url https://rpc.monad.xyz | grep -E '^(status|blockNumber)'
blockNumber          101676008
status               1 (success)
```

## 3. Verificación offline del recibo

`/dx402/receipt/{paymentId}` devuelve todo lo que hace falta: el struct con los 9 campos, la firma, el `signer` que el facilitador declara y el dominio (`name`, `version`, `chainId` de la cadena de liquidación). Nada faltó; no hubo que inventar ningún campo.

```bash
$ curl -s https://facilitator.ultravioletadao.xyz/dx402/receipt/0x96d9a8fbe441d2fd8abd3628d14644582d8f3b926f2a223edb0a5b75e8f9761f
{"receipt":{"paymentId":"0x96d9…761f","contentHash":"0xbca4…f93c","pointer":"ipfs+https://facilitator.ultravioletadao.xyz/dx402/blob/0x96d9…761f#bafkreifc22…74qi","payer":"0x09C32b8FC0a94A1EeD424499A42180e29667bEeE","payee":"0x64dbE996E626260F21F5c4FaD3C9bA209978c368","txHash":"0x9aa2…c4cf","network":"base","mode":"direct","anchoredAt":1788467786,"retentionUntil":1796243786},"signature":"0xfffe84ac…6661c","signer":"0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF","domain":{"name":"DX402 Evidence","version":"1","chainId":8453}}
```

El script (`eth_account` 0.13.7). El orden de campos es el de `src/dx402/receipt.rs:34-44` — entra en el `typeHash`, así que reordenarlo invalida cada recibo emitido. `mode` se codifica como `EvidenceMode::as_u8` (`src/dx402/types.rs:47`: `direct`=0, `escrowed`=1). Payer y payee no-EVM van como la dirección cero (`receipt.rs:80`).

```python
# verify_receipt.py  --  python3 verify_receipt.py <paymentId>
import json, sys, urllib.request
from eth_account import Account
from eth_account.messages import encode_typed_data

j = json.load(urllib.request.urlopen(
    f"https://facilitator.ultravioletadao.xyz/dx402/receipt/{sys.argv[1]}"))
r, dom = j["receipt"], j["domain"]
types = {"Dx402EvidenceReceipt": [
    {"name": "paymentId",      "type": "bytes32"},
    {"name": "contentHash",    "type": "bytes32"},
    {"name": "pointer",        "type": "string"},
    {"name": "payer",          "type": "address"},
    {"name": "payee",          "type": "address"},
    {"name": "txHash",         "type": "bytes32"},
    {"name": "mode",           "type": "uint8"},
    {"name": "anchoredAt",     "type": "uint64"},
    {"name": "retentionUntil", "type": "uint64"},
]}
MODE = {"direct": 0, "escrowed": 1}
msg = {
    "paymentId":   bytes.fromhex(r["paymentId"][2:]),
    "contentHash": bytes.fromhex(r["contentHash"][2:]),
    "pointer":     r["pointer"],
    "payer":       r["payer"],
    "payee":       r["payee"],
    "txHash":      bytes.fromhex(r["txHash"][2:]),
    "mode":        MODE[r["mode"]],
    "anchoredAt":  r["anchoredAt"],
    "retentionUntil": r["retentionUntil"],
}
signable  = encode_typed_data(domain_data=dom, message_types=types, message_data=msg)
recovered = Account.recover_message(signable, signature=j["signature"])
print("network      :", r["network"], "chainId", dom["chainId"])
print("signer (API) :", j["signer"])
print("recovered    :", recovered)
print("MATCH" if recovered.lower() == j["signer"].lower() else "MISMATCH")
```

Salida, sobre dos cadenas distintas (dos `chainId` distintos en el dominio):

```
$ python3 verify_receipt.py 0x96d9a8fbe441d2fd8abd3628d14644582d8f3b926f2a223edb0a5b75e8f9761f
network      : base chainId 8453
signer (API) : 0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF
recovered    : 0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF
MATCH

$ python3 verify_receipt.py 0x7e7ca6a10e0d2a835fd0d8045d0a95a6ee5f5dd29b39696e69c3c21c7a3387ec
network      : avalanche chainId 43114
signer (API) : 0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF
recovered    : 0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF
MATCH
```

Un tercero no necesita al facilitador para esto: con el JSON del recibo y la dirección `0x7bC4…4e0DF` publicada, la recuperación es local.

## 4. Lo que este corpus NO prueba

- **Un operador, un marketplace.** Los 116 anclajes EVM verificados salen de la flota de KarmaCadabra operando sobre Execution Market. Los 26 compradores y 24 vendedores distintos son wallets de esa flota, no 50 partes independientes. No hay todavía **ningún vendedor de terceros** que haya anclado evidencia.
- **Los 5 `verified: true` de Solana no están verificados contra la cadena, y tres de sus `txHash` no son firmas de Solana** (`KKFIRMADEMO…`, `KKBIDI…`, `KKCUSTODIA…`; los otros dos son hex de 64 sin `0x`). Son los 5 del 2026-08-18, anteriores a la escalera de autoridad de v1.82.0. El código actual no puede producirlos: `evaluate_gate` devuelve `UnverifiableChain` para toda familia no-EVM (`src/dx402/service.rs:441-447`) y `verified = gate_verdict.is_none()` (`:542`), así que hoy un anclaje de Solana sale `verified: false` con `notVerifiedReason: dx402_unverifiable_chain`. **Esas 5 filas hay que corregirlas o excluirlas antes de citar el corpus**; este documento las cuenta aparte por eso. Con ellas fuera, los números honestos son **116 verificados en 7 redes EVM**.
- **No-EVM en general**: los anclajes de NEAR/Stellar/Algorand/Solana pueden quedar `signed` (firma ed25519 del payee, v1.82.0) pero nunca `verified`; el gate no lee esas cadenas. En este corpus no hay ninguno `signed` no-EVM.
- **La fase 2 no está encendida.** Todo lo verificado se verificó en fase 1 (`DX402_REQUIRE_PROOF=false`): el gate corre y reporta, no rechaza. Nadie ha medido todavía que tráfico legítimo pase con el gate imponiendo.
- **"Sostenido 7 días" no se cumple.** El rango va del 2026-08-18 al 2026-09-03, pero 122 de los verificados se concentran en el 2026-09-03/04 y la flota quedó pausada el 2026-09-04 02:09Z (`09-ESTADO-Y-CAMINO-A-UPSTREAM.md` §2).
- Lo que sí prueba: que la secuencia completa —settle real → anclaje provisional → firma del payee dentro de la ventana → verificación contra el recibo on-chain → recibo EIP-712 recuperable offline— corrió 116 veces en 7 redes EVM con un firmante estable.

## 5. Cómo reproducirlo

```bash
# 0. Estado del facilitador
curl -s https://facilitator.ultravioletadao.xyz/version
curl -s https://facilitator.ultravioletadao.xyz/supported | jq .extensions      # ["bazaar","durable-evidence"]
curl -s https://facilitator.ultravioletadao.xyz/dx402/stats | jq '{anchored,receiptSigner,note}'

# 1. Corpus (requiere credenciales AWS con lectura sobre la tabla)
aws dynamodb scan --table-name facilitator_dx402_evidence --region us-east-2 --output json > scan.json
python3 - <<'EOF'
import json, collections, datetime
TEST = "0x1111111111111111111111111111111111111111"
rows = []
for it in json.load(open("scan.json"))["Items"]:
    r = json.loads(it["record"]["S"]); rc = r["receipt"]
    if TEST in (rc["payer"], rc["payee"]): continue
    rows.append(dict(verified=it.get("verified", {}).get("BOOL", False), signed=it.get("signed", {}).get("BOOL", False),
                     network=rc["network"], payer=rc["payer"].lower(), payee=rc["payee"].lower(), tx=rc["txHash"], at=rc["anchoredAt"]))
ver = [x for x in rows if x["verified"]]; evm = [x for x in ver if x["network"] != "solana"]
f = lambda t: datetime.datetime.fromtimestamp(t, datetime.UTC).isoformat()
print("anchors", len(rows), "verified", len(ver), "verified_evm", len(evm), "signed", sum(x["signed"] for x in rows))
print("per-network verified", collections.Counter(x["network"] for x in ver).most_common())
print("payers", len({x["payer"] for x in ver}), "payees", len({x["payee"] for x in ver}),
      "| evm-only payers", len({x["payer"] for x in evm}), "payees", len({x["payee"] for x in evm}))
print("first/last verified", f(min(x["at"] for x in ver)), f(max(x["at"] for x in ver)))
print("verified with non-EVM txHash", [x["tx"] for x in ver if not (x["tx"].startswith("0x") and len(x["tx"]) == 66)])
EOF

# 2. Muestra: evidencia + recibo por paymentId (sin credenciales)
for id in 0x7e7ca6a10e0d2a835fd0d8045d0a95a6ee5f5dd29b39696e69c3c21c7a3387ec \
          0x96d9a8fbe441d2fd8abd3628d14644582d8f3b926f2a223edb0a5b75e8f9761f; do
  curl -s https://facilitator.ultravioletadao.xyz/dx402/evidence/$id | jq -c '{verified,signed,receiptSigner,backend,keyAlg}'
  curl -s https://facilitator.ultravioletadao.xyz/dx402/receipt/$id  | jq -c '{network:.receipt.network,txHash:.receipt.txHash,chainId:.domain.chainId}'
done

# 3. Recibo offline (pip install eth-account) -- script en la sección 3
python3 verify_receipt.py 0x96d9a8fbe441d2fd8abd3628d14644582d8f3b926f2a223edb0a5b75e8f9761f

# 4. La transacción existe y fue exitosa (foundry `cast`)
cast receipt 0x9aa235ef1ffeacf1f8a2731d208a8a959c452f90be1c84262385d71022a0c4cf --rpc-url https://mainnet.base.org | grep -E '^(status|blockNumber)'
cast receipt 0x36afe04eba6bd1e17215c21e0b2307b96c04934d088418032100f755b9d07ec8 --rpc-url https://rpc.monad.xyz    | grep -E '^(status|blockNumber)'
```
