# Simular un release de escrow para probar el gate hasta el final

**Por que.** El escalon 2 del anchor (`verified`) exige un pago de menos de 900
segundos. El riel de escrow puede pasar dias sin uno — el 2026-09-03 llevaba
**23,6 horas** sin un solo release, con el enjambre apagado. Sin esto, la unica
forma de comprobar que el gate llega hasta el final es esperar a que alguien
compre algo, y *"no lo pude probar porque no habia trafico"* no es una
verificacion.

**Que es y que no.** No es un mock. Se forkea la cadena real un bloque antes de
un release de Execution Market que ya ocurrio y se **reejecuta la misma
transaccion**, de modo que caiga en un bloque con timestamp de ahora. El codigo
del escrow y del PaymentOperator son los desplegados, el `paymentInfoHash` lo
calcula el contrato, y la firma la produce el custodio real del payee. **Lo unico
que la simulacion mueve es el reloj.**

La firma es la parte que no se puede fingir: tiene que recuperar al payee que la
cadena reporta. Por eso hay que elegir un release cuyo payee sea una wallet
nuestra — en el ejemplo, `kk-karma-hello`.

## Receta

```bash
# 1. Elegir un release real y forkear UN BLOQUE ANTES
TX=0x31cfb069394330153669fbf109e1823dc7fbd8b0d6de94e683c4dd1ddc0f1a87   # monad
anvil --fork-url https://rpc.monad.xyz --fork-block-number 101220018 --port 8545 --silent &

# 2. Reejecutar la transaccion con timestamp de AHORA
FAC=0x103040545ac5031a11e8c03dd11324c7333a13c7      # quien la mando (el facilitador)
OP=0x9620dbe2bb549e1d080dc8e7982623a9e1df8cc3       # PaymentOperator de monad
INPUT=$(cast tx $TX --rpc-url https://rpc.monad.xyz --json | jq -r .input)
cast rpc anvil_impersonateAccount $FAC --rpc-url http://127.0.0.1:8545
cast rpc anvil_setBalance $FAC 0xde0b6b3a7640000 --rpc-url http://127.0.0.1:8545
cast rpc evm_setNextBlockTimestamp $(date +%s) --rpc-url http://127.0.0.1:8545
cast send --unlocked --from $FAC $OP $INPUT --rpc-url http://127.0.0.1:8545
```

3. Armar la reclamacion firmada con el barrido de KarmaCadabra
   (`agents_sdk/dx402_anclajes.py`), apuntando `_rpc` al fork y firmando con
   Paybox. Volcarla a un JSON con las claves `rpc, pid, contentHash, pointer,
   sealedTo, payee, sig, proof, escrowRelease`.

4. Correr el gate:

```bash
DX402_SIM=/ruta/sim.json cargo test --locked -p x402-rs \
  --features solana,near,stellar,algorand,sui,xrpl \
  --test dx402_escrow_sim -- --nocapture
```

Sin `DX402_SIM` el test se salta solo, asi que en CI no pide red ni anvil.

## Lo que quedo medido (2026-09-03, monad)

| | |
|---|---|
| Release reejecutado | `status 0x1`, 4 logs, bloque a **22s** |
| `getHash` del escrow | `0xe53e19d549c9a4a8…` — lo calculo el contrato |
| Firma de Paybox | recupera a `0xa3279F74…`, el payee |
| `verify_anchor` | **`Ok(())`** — escalon 2 alcanzado |
| Sin `escrowRelease` | `dx402_escrow_release_missing` |
| Con firma falsa | `SellerSignatureInvalid` |

Los dos ultimos importan tanto como el primero: prueban que el gate **rechaza**,
y por lo tanto que el `Ok` de arriba significa algo.

## El limite honesto

Esto prueba que el gate llega al escalon 2 con datos reales de escrow. **No
produce un `verified: true` en la tabla de produccion**, y no deberia: el gate
lee la cadena de verdad, y un anclaje de produccion tiene que venir de un pago de
produccion. Para eso hace falta trafico real dentro de la ventana de 900s — o
sea, el enjambre encendido.
