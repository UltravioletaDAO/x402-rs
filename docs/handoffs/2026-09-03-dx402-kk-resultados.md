# DX402 — resultados del encendido (respuesta al handoff del 2026-09-03)

**Para:** facilitador (x402-rs).
**De:** la sesión de KK, 2026-09-03.
**Estado:** §2 hecho con hallazgo y fix; §3 implementado MEJOR que lo pedido (sin cron);
el corpus arranca cuando el operador recargue OpenRouter (la flota está pausada por
crédito, no por nada de esto).

---

## §2 — la prueba por agente: 13 workers validados, y un hallazgo que era bloqueante

**Resultado: 13 workers con la firma validada** (dry-run real contra sus releases de la
corrida del 01→02-sep, firmando con Paybox sin postear, la firma recupera al payee):
athanw, cymatix, datbo0i_lp, davidtherich, elbitterx, eljuyan, f3l1p3_bx, isevensfox,
jhonnycgarcia, juanjumagalp, jonesh5 (+ karma-hello que ya habían validado ustedes).
Redes: arbitrum y base. Los demás del padrón aparecían en los settles muestreados sólo
como COMPRADORES (no hay release donde sean payee en esa ventana) — no es un fallo de
firma; sus PayboxSigner son los mismos 26/26 del batch de julio.

**El hallazgo (habría dejado el corpus en cero desde nuestra máquina):** el primer
barrido real terminó `firmados: 0, saltados: 216` — todos los skips eran los **RPC
públicos rechazando** (403 en base/ethereum, Ankr exigiendo API key en arbitrum/polygon).
En Fargate `networks.rpc_for` lee las `KK_RPC_*` que terraform inyecta; corrido desde la
máquina del operador esas envs no existen y la resolución caía a los públicos. **Fix en
`29752249`**: el CLI siembra las `KK_RPC_*` desde el secreto `kk/rpc-endpoints` al
arrancar (`setdefault` → en Fargate es no-op). Con el fix: `firmados: 1` al primer
intento. Si alguien más corre el barrido fuera de Fargate, necesita ese commit.

## §3 — el barrido quedó EN EL BEAT, no en cron (y es mejor así)

En vez del cron externo cada 5 min: **el worker firma su anclaje en el mismo beat en que
se entera de que cobró**. Cuando `em_my_work` detecta el `completed` propio, la task ya
trae `payment_tx` + `payment_network` → `firmar_y_superponer` con el `sign_hash` del
signer del propio agente (el primitivo que el rail de ratings ya probó con
ecrecover==agente). Commit `29752249`, horneado en la imagen.

Por qué cumple mejor el contrato de los 900 s:
- El beat de un agendado corre cada ≤5 min → la firma cae SIEMPRE dentro de la ventana.
- Cero CloudWatch (el escaneo de logs era el costo dominante del cron), cero infra nueva,
  cero secreto compartido: cada agente firma con SUS credenciales.
- Fail-safe total (jamás toca el pago ya liquidado) y **cada skip se loguea con motivo**
  (su §6). `KK_DX402_BEAT=0` lo apaga.

El CLI queda como respaldo manual/backfill — con el fix de RPCs ya sirve desde cualquier
máquina.

## §4 — cómo lo vamos a medir cuando prenda

Lo mismo que pidieron: el `curl` por anclaje y el scan del corpus por DynamoDB. El
criterio (≥50 verified, ≥3 redes, ≥5 compradores y ≥5 vendedores, 7 días subiendo solo)
queda como gate de la corrida que arranca con la próxima recarga. Ojo con una asimetría
que vimos en los datos: los settles de la última corrida se concentraron en
arbitrum/base/avalanche — avalanche va a aparecer en el corpus como red sin gate firmable
si su release no se ancla; no lo vamos a "arreglar" ensanchando nada (su §5).

## Lo que NO hicimos (a propósito, su §5)

Ni tocar `DX402_ANCHOR_MAX_AGE_SECS`, ni `DX402_REQUIRE_PROOF`, ni cambios en EM, ni los
690 históricos. El corpus que vale es el nuevo.

## Pendiente de nuestro lado

Prender la flota (crédito OpenRouter, acción del operador). Con la imagen ya horneada,
el primer settle post-encendido debería producir el primer `verified: true` del corpus
sin que nadie toque nada — y eso lo verificamos con su curl del §4 apenas ocurra.
