# DX402 — el corpus certificado quedó lleno: 122 verificados (meta 50)

**Para:** facilitador (x402-rs), para el PR de `durable-evidence` a la x402 Foundation.
**De:** la sesión de KK, 2026-09-04.
**Estado:** el criterio de su handoff del 2026-09-03 («verified ≥ 50, en ≥ 3 redes, ≥ 5
compradores y ≥ 5 vendedores») está cumplido con margen. Lo único que no está es «sigue
subiendo solo durante 7 días»: la flota quedó pausada por crédito el 2026-09-04 02:09Z y
se reanuda al recargar (mañana hay demo).

## 1. Los números, medidos sobre la tabla (no sobre logs nuestros)

```bash
aws dynamodb scan --table-name facilitator_dx402_evidence --region us-east-2 --select COUNT
aws dynamodb scan --table-name facilitator_dx402_evidence --region us-east-2 --select COUNT \
  --filter-expression "verified = :t" --expression-attribute-values '{":t":{"BOOL":true}}'
```

| | |
|---|---|
| evidencias en la tabla | 827 |
| `verified: true` | **122** |
| redes con verificados | **8**: avalanche 49 · arbitrum 37 · optimism 9 · base 8 · monad 7 · ethereum 6 · solana 5 · polygon 1 |
| compradores distintos (`receipt.payer`) | **31** |
| vendedores distintos (`receipt.payee`) | **29** |
| anclados entre | 2026-08-18 19:22Z y 2026-09-03 23:24Z |
| `mode` | `direct`, backend `ipfs`, pointer `ipfs+https://facilitator…/dx402` |

Los 705 no verificados son mayoritariamente anclajes provisionales de EM sin superponer
(`dx402_seller_signature_missing`) y pruebas vencidas de la primera semana; no los tocamos.

## 2. Cómo llegó ahí (por si el PR quiere contar el camino)

- **El anclaje va en el beat, no en cron** (`agents_sdk/tools.py::_dx402_anclar_settle`,
  handoff del 03): cada `em_my_work` del worker firma y superpone su evidencia sobre el
  anclaje provisional de EM apenas ve el settle. Con la flota viva el contador sube solo.
- **13 workers validados** en dry-run contra sus releases antes de prender (handoff del 03).
- **Dos arreglos nuestros** que sin ellos el corpus quedaba en cero desde Fargate: el RPC
  de los anclajes va por el proxy SigV4 primero (arbitrum daba 403 en Fargate), y la CLI
  `scripts/kk/dx402_firmar_anclajes.py` siembra los RPC privados de `kk/rpc-endpoints`.
- **La lectura del facilitador es trazable de punta a punta** desde el 2026-09-03: toda
  llamada a un endpoint deja una traza que el observatorio pinta (`events.trace_ctx`), y
  el anillo del observatorio muestra las **21 mainnets de `/supported`**, con cada settle
  aterrizando en su placa y regresando. Es material de demo para el PR y para la reunión.

## 3. Qué vimos del riel esta corrida (8 h, 2475 beats, 119 liquidaciones con hash)

- **Nada roto del lado del facilitador.** Los fallos de la corrida son de EM o nuestros:
  66 `escrow lock failed` (misma señal de Ethereum de siempre), 134 × 422 por ids
  truncados (nuestro), y 4 approves de Optimism trabados por **502 en el settle de EM**
  (`0ae3f297`, `708bfb3d`, `1143097e`, `1f2c3440`, ~$0.05). Los reintentamos cuando EM lo cierre.
- `/supported` se lee al cargar el observatorio para los decimales por asset; funcionó
  las 8 horas.

## 4. Lo que les pedimos

1. Confirmen si el PR necesita los 7 días de crecimiento continuo o si 122/8 redes/31/29
   alcanza como corte. Si necesitan los 7 días, la flota vuelve a correr apenas se
   recargue el crédito (mañana en modo demo, después normal).
2. Si van a citar el corpus, el comando de conteo del §1 es reproducible desde cualquier
   máquina con AWS; los `payment_id` son públicos y se pueden listar con la misma tabla.

## 5. Lo que NO hicimos (a propósito, su §5 del 03)

- No encendimos `DX402_REQUIRE_PROOF=true`. Fase 2 sigue siendo de ustedes.
- No tocamos evidencias no verificadas ni retenciones.
