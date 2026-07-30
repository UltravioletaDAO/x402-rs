# Backlog — backfill histórico de pagos x402 on-chain

**Estado:** BLOQUEADO por una API key. El código está escrito, probado y funcionando.
**Desbloquea:** una `ETHERSCAN_API_KEY` con cobertura de las cadenas que nos importan.

---

## Qué está hecho

`scripts/scan/scan_evm.py` y `scripts/scan/run_all.py` están implementados y
verificados contra la cadena. Un comando, sin razonamiento de agente:

```bash
export ETHERSCAN_API_KEY=...
python scripts/scan/run_all.py
```

Las redes salen de `config/supported_tokens.json`, así que agregar una cadena al
facilitador la agrega al escaneo sin tocar los scripts.

## Por qué esto no es "contar transacciones de la wallet"

La wallet del facilitador hace mucho más que liquidar pagos: recarga gas, mintea
identidades ERC-8004, escribe reputación, libera y reembolsa escrow. El escaneo
filtra por **selector de método** — `transferWithAuthorization` es `0xe3ee160e`,
leído de un settle real en Base el 2026-07-29 y confirmado contra el recibo.

Ethereum lo demuestra solo: **88 transacciones de la wallet, exactamente 1 pago
x402.** Reportar 88 habría sido un número verdadero contestando otra pregunta.

## El bloqueo, medido el 2026-07-30

| Vía | Estado |
|---|---|
| Hosts V1 (`api.basescan.org` y familia) | **MUERTOS** — responden "deprecated V1 endpoint" |
| Etherscan V2 (`api.etherscan.io/v2/api?chainid=N`) | Exige API key **incluso para Ethereum** |
| Etherscan V2, plan gratuito, Base | **No cubierto** — "upgrade your api plan" |
| Blockscout | Gratis y sin key, pero cobertura despareja: `eth.blockscout.com` responde 200, `base.blockscout.com` da 500 |

Resultado actual sin key: **1 de 25 redes escaneadas**, vía Blockscout de Ethereum.
Las otras 24 se reportan **UNSCANNED, nunca como cero** — esa distinción es todo
el diseño del script, y por eso `run_all.py` sale con código distinto de cero
ante resultados parciales, para que un cron no archive un reporte incompleto
como éxito.

## Qué hace falta decidir

Base es nuestra cadena de mayor volumen y el plan gratuito de Etherscan no la
cubre. Así que es una decisión de plan pago, no de ingeniería:

1. **Plan pago de Etherscan** — una key cubre toda la familia V2. Lo más simple.
2. **Blockscout por cadena** — gratis, pero hay que verificar host por host y
   varios no responden. Agregar solo instancias **confirmadas funcionando**: una
   adivinada se convierte en un cero silencioso.
3. **Aceptar cobertura parcial** — escanear lo que se pueda y etiquetar el resto
   como desconocido. Ya es el comportamiento por defecto.

## Cuando se desbloquee

1. `export ETHERSCAN_API_KEY=...`
2. `python scripts/scan/run_all.py`
3. Los reportes quedan en `docs/reports/onchain-scan/`
4. Los números del barrido **no se suman** a los de `/api/stats`: uno cubre toda
   la historia pero solo ve lo que la cadena muestra, el otro cubre solo desde
   que se encendió el store pero ve cada operación, verifies incluidos.
   Mezclarlos produce un total que no responde ninguna pregunta.

## Pendiente aparte: familias no-EVM

El escaneo cubre EVM. Solana, SUI, NEAR, Stellar, Algorand y XRPL necesitan cada
una su script — misma forma, misma regla: filtrar por lo que realmente es un
pago, y reportar lo inalcanzable como desconocido y no como cero.
