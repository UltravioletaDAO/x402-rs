# DX402 — Encender el enjambre para llenar el corpus certificado

**Para:** la sesión de KarmaCadabra.
**De:** facilitador (x402-rs), 2026-09-03.
**Objetivo:** que cada trade del enjambre sobre Execution Market termine en un
anclaje DX402 con **`verified: true`**, sin intervención manual, durante ≥7 días.
Eso es lo único que separa a `durable-evidence` de un PR a la x402 Foundation
que no se descarte por falta de uso.

---

## 0. Por qué esto y por qué ahora

Al 2026-09-03 hay **699 anclajes en producción, 1 firmado, 0 verificados sobre
trades reales**. No porque algo esté apagado: en el riel de escrow **EM no puede
firmar** (el payee del release es el worker, no EM), y hasta el facilitador
2.10.0 el gate leía al comprador del `from` del Transfer, que en un release es el
TokenStore del operador — 23 de 23 releases muestreados fallaban. Las dos cosas
están arregladas y probadas por partes, incluido el gate hasta el escalón final
sobre un release real reejecutado en fork. **Lo que no ha corrido nunca es la
secuencia completa sobre un pago real**, porque el enjambre lleva >24h apagado
(último release: 2026-09-02 04:04 UTC).

La secuencia:

```
trade de KK en EM  →  release de escrow (on-chain)
                   →  EM ancla PROVISIONAL (ya lo hace, sin cambios)
                   →  el WORKER firma dentro de 900s y superpone   ← esto es lo nuevo
                   →  el facilitador verifica contra la cadena     → verified: true
```

## 1. Lo que ya está en tu repo (`master` = `261df4f8`)

| Archivo | Qué es |
|---|---|
| `agents_sdk/dx402_anclajes.py` | La librería: lee el release, reconstruye la autorización del escrow, arma la prueba de pago, firma con Paybox y superpone. **No postea nada que la cadena no respalde**: valida el `paymentInfo` con `getHash` del propio escrow y recupera la firma localmente antes de mandar |
| `scripts/kk/dx402_firmar_anclajes.py` | El CLI. `--agente`, `--wallet`, `--tx RED:HASH` o `--horas N`, `--dry-run`, `--sin-firmar` |
| `tests/sdk/test_dx402_anclajes.py` | 7 tests sobre el release real `0x5a2822cc…` de optimism |

Requiere `uvd-x402-sdk[dx402]` (ya en `requirements-sdk.txt`) y facilitador
≥ 2.10.0 (en producción).

## 2. Antes de prender: una prueba de 2 minutos por agente

Por cada agente que actúe como **worker** en EM (el que cobra; los compradores no
firman nada), un dry-run contra cualquiera de sus releases viejos. Firma de
verdad con Paybox pero no postea:

```bash
python scripts/kk/dx402_firmar_anclajes.py \
  --agente kk-karma-hello \
  --wallet 0xa3279F744438F83Bc75ce9f8A8282c448F97cc8A \
  --tx monad:0x31cfb069394330153669fbf109e1823dc7fbd8b0d6de94e683c4dd1ddc0f1a87 \
  --dry-run
```

Salida buena: `LISTO monad ... (validado y firmado, sin postear)` y el JSON del
anclaje. Ya lo corrí para `kk-karma-hello`: Paybox firma el digest crudo y la
firma **recupera al payee** (si aplicara EIP-191 recuperaría a otra dirección y
el anclaje quedaría provisional para siempre, sin error).

Si sale `skip ... la firma recupera a 0x…, no a 0x…`: la `--wallet` no es la que
firma ese `credential_id`. Revisá el secreto `kk/<agente>` (**us-east-1**, no
ca-central-1 — ese es el de los logs).

Wallets del catálogo directo, por si sirven de referencia: `kk-karma-hello`
`0xa3279F74…`, `kk-validator` `0x7a729393…`. Las demás salen del secreto o del
`AgentContext`.

## 3. Prender

1. Enjambre al ritmo normal. **Nada especial**: DX402 en EM está encendido por
   defecto (`EM_DX402_ENABLED`), y EM ancla solo.
2. **El barrido en cron, cada 5 minutos, por cada worker.** La ventana de
   frescura del gate son **900 s**; a los 15 min el pago "vence" y el anclaje
   queda provisional aunque la firma sea perfecta.

```bash
# cada 5 min, por agente worker
python scripts/kk/dx402_firmar_anclajes.py \
  --agente <agente> --wallet <su EVM> --horas 0.2
```

`--horas` escanea las líneas `[SETTLE]` de la flota (CloudWatch, `ca-central-1`,
prefijo `/ecs/karmacadabra-sdk-canada/`) y prueba cada tx en cada red del escrow;
en la red que no es, no hay receipt y sale un skip silencioso. Con más agentes o
más redes eso escala lineal; si molesta, filtrá `--redes` a las que EM use.

Un skip que **sí** hay que mirar: `no hay evidencia anclada para este pago` en
un release fresco = **EM no ancló**. Eso es de EM, no del barrido.

## 4. Cómo sabemos que está

Por anclaje:

```bash
curl -s https://facilitator.ultravioletadao.xyz/dx402/evidence/<paymentId> | jq '{verified,signed,notVerifiedReason}'
```

`{"verified": true, "signed": true}` es el objetivo. `verified: false` con
`notVerifiedReason` te dice exactamente en qué escalón se cayó:

| `notVerifiedReason` | Qué es |
|---|---|
| `dx402_proof_missing` | no llegó `proofOfPayment` — el barrido no corrió sobre ese pago |
| `dx402_seller_signature_missing` | anclaje provisional de EM, todavía sin superponer |
| `dx402_proof_invalid` | la prueba se leyó y no pasó. El caso esperable acá es **vencida**: el barrido llegó después de los 900 s → **acortá el cron**. El motivo fino (`proof_expired`, etc.) queda en el log del facilitador, no en el API |
| `dx402_escrow_release_missing` | riel de escrow sin `escrowRelease` — no debería pasar con este barrido |
| `dx402_rpc_unavailable` | el facilitador no pudo leer la cadena; reintenta solo |

Por corpus (desde una máquina con AWS):

```bash
aws dynamodb scan --table-name facilitator_dx402_evidence --region us-east-2 --output json \
 | python3 -c "import sys,json,collections as c;I=json.load(sys.stdin)['Items'];print('total',len(I));print('verified',c.Counter(i.get('verified',{}).get('BOOL') for i in I));print('signed',c.Counter(i.get('signed',{}).get('BOOL') for i in I))"
```

**Listo cuando:** `verified: True ≥ 50`, en ≥3 redes, ≥5 compradores y ≥5
vendedores distintos, y el contador sigue subiendo solo durante 7 días.

## 5. Lo que NO hace falta y lo que NO tocar

- **No** ensanchar `DX402_ANCHOR_MAX_AGE_SECS` para "arreglar" los 690
  históricos. El corpus que vale es el nuevo. Ensanchar una ventana de seguridad
  para que cierre una métrica es exactamente el movimiento contra el que está
  escrito el gate.
- **No** hace falta cambiar EM. El barrido toma todo de la cadena y de
  `GET /dx402/evidence`.
- **No** encender `DX402_REQUIRE_PROOF=true` todavía. Fase 2 va después de que
  los logs muestren tráfico real pasando ≥48 h.
- La bidireccionalidad (sobre multi-destinatario con copia para el vendedor) es
  un ítem de **EM** (`seller_encryption_key` en `anchor.py`), no de este barrido.
  No bloquea el corpus.

## 6. Si algo se rompe, dónde mirar

- Facilitador: `docs/DX402.md` (guía), `docs/plans/dx402/08-SPEC-v0.2.md` (spec),
  `09-ESTADO-Y-CAMINO-A-UPSTREAM.md` (seguimiento con evidencia).
- Reproducir el gate sin esperar tráfico: `docs/plans/dx402/07-SIMULACION-ESCROW.md`.
- El barrido loguea cada skip con su motivo; **un skip callado es como este
  agujero se quedó cuatro meses**, así que no los filtres.
