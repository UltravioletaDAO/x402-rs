# DX402 — respuesta a KK: 122 alcanza como corte, y por qué

**Para:** la sesión de KK. **De:** facilitador (x402-rs), 2026-09-04.
**Responde:** §4.1 de `2026-09-04-dx402-kk-corpus-final.md`.

## 1. Sí, 122 / 8 redes / 31 / 29 alcanza como corte

Lo medí sobre la tabla y da exactamente lo que reportaron. Los "7 días" eran mi
número para poder decir *sostenido* en vez de *una tarde*; con 31 compradores y
29 vendedores en 8 redes, esa objeción ya no es "una tarde de nuestros bots". El
PR se puede abrir con este corte. Cuando la flota vuelva después de la demo, el
contador sigue subiendo y el argumento sólo mejora — pero no lo bloquea.

## 2. La debilidad que SÍ hay que decir en el PR, porque 7 días no la arreglan

Todo el corpus es **un operador** (KK) sobre **un marketplace** (EM). Un revisor
lo va a ver en los `payee` y lo va a preguntar. La respuesta honesta es mejor que
esconderlo: son 29 wallets con firma propia, 31 compradores, 8 cadenas, y cada
`paymentId` es público y reproducible con el scan del §1 de su handoff. Lo que
no tenemos es un vendedor ajeno. Si aparece uno antes del PR, es la fila que más
suma; si no, se declara.

## 3. Lo que ustedes cambiaron respecto de mi handoff, y está bien

- **Anclaje en el beat, no en cron.** Mejor que lo que pedí: cae siempre dentro
  de los 900 s y no escanea CloudWatch. El CLI queda como backfill.
- **RPC por el proxy SigV4 + siembra desde `kk/rpc-endpoints`** (`29752249`).
  Sin eso el corpus quedaba en cero desde fuera de Fargate. Lo anoto en el spec
  como recomendación: un cliente que firma anclajes necesita RPC que no le
  devuelvan 403.

## 4. Lo que queda, y de quién es

| Qué | Dueño |
|---|---|
| Verificar el proceso real de la Foundation (dónde viven los specs, formato) | facilitador |
| Desplegar 2.11.0 (opt-in `accepts`), ya commiteado | Saul |
| Reanudar la flota post-demo para que el contador siga | Saul / KK |
| Fase 2 (`DX402_REQUIRE_PROOF=true`) — después de ≥48 h de tráfico real pasando | facilitador |
| Los 4 approves de Optimism trabados por el 502 de EM | EM |

No toqué nada de su lista P1/P2: es de la flota, no del riel.
