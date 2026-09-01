---
date: 2026-09-01
tags:
  - type/handoff
  - domain/performance
  - domain/erc8004
  - priority/p0
status: active
---

# El p99 de 11,4s: un arreglo que optimizó una rama que nunca corría

> **Disparador:** alertas de `facilitator-production-latency-p99` y
> `-latency-p99-early` toda la noche del 31-ago al 1-sep.
> **Estado:** arreglado en la rama `investigate/p99-latency`, worktree
> `/home/zeroxultravioleta/x402-p99`. **Sin desplegar.**

## La frase que resume todo

**`totalSupply()` revierte en todos los registries ERC-8004 desplegados.** El
arreglo del 29-ago la puso primero y dejó la sonda secuencial como *fallback*.
Como la llamada siempre falla, el fallback fue **el único camino que corrió
nunca**: el cambio fue un no-op en producción durante toda su vida, y el p99 se
quedó exactamente donde estaba.

## Qué se midió

| Dato | Valor | Cómo |
|---|---|---|
| p99 sostenido | **11,4s** (p50 0,048s, **p90 4,7s**) | CloudWatch, 16h |
| Inicio | 31-ago 19:55 UTC, sin recuperarse | CloudWatch 5-min |
| Ruta culpable | `/identity/{network}/owner/{address}` | 1223 req/30min, la dominante |
| Costo por lookup frío en celo | **28 `eth_call` en serie** | `scripts/erc8004_registry_capabilities.py` |
| `totalSupply()` en celo y base | **revierte**; `supportsInterface(ERC721Enumerable)` = false | `eth_call` directo |
| Máximo de agentes: celo | 9.802 | sonda binaria |
| Máximo de agentes: base | **83.984** | sonda binaria |
| 404 de esa ruta en 6h | 5.524, **máximo 383ms** | logs ECS |

28 llamadas × ~400ms contra el RPC de producción = 11,2s. Medido en prod:
11,12s y 11,20s en dos llamadas consecutivas. La aritmética cierra exacta.

El disparo no fue un deploy: el tráfico pasó de ~60 a ~130 req/5min a las 19:55
(un cliente barriendo owner-lookups en nueve redes). **La ruta estuvo rota todo
el tiempo; la carga la hizo visible.**

## Por qué no lo vio nadie

1. **El log que lo delataba era `debug!`** y producción corre en `info`. La
   única línea que decía "el camino rápido falló, me fui al lento" nunca se
   imprimió.
2. **1.264 tests verdes no tocaban la rama de producción.** `registry_total_supply`
   no aparecía en ningún test. El único camino que corría era el único sin
   cobertura.
3. **`/identity/{network}/total-supply` venía devolviendo 501 en las nueve
   redes.** El commit lo citó como prueba de que `totalSupply()` servía
   ("ya estaba en el ABI y ya la usaba este endpoint") — cierto sobre el ABI,
   falso sobre la cadena. Nadie llamó al endpoint para enterarse.
4. **Los 404 son rápidos** (43ms, atajo por `balanceOf == 0`), así que la mitad
   del tráfico se veía sana.

## Qué se cambió

Todo en `src/handlers.rs`.

**Rendimiento**
- `discover_max_agent_id`: escalera exponencial + refinamiento k-ario, todo por
  **Multicall3**. De ~28 idas y vueltas *en serie* a **≤4 en paralelo**, para
  cualquier registry que la escalera describa.
- `REGISTRY_BOUND_CACHE`: el máximo del registry es un número **igual para todos
  los llamadores** y sólo crece; se cacheaba por owner, nunca el máximo. Ahora
  un lookup tibio cuesta **0** idas y vueltas de descubrimiento.
- `scan_range_for_owner` corre en **olas concurrentes acotadas** (`OWNER_SCAN_WAVE = 4`).
  Base necesita 42 batches: en serie eran los 3-9s medidos.
- `ScanOrder`: cuando `balanceOf == 1` —todo el tráfico actual— **el único match
  que existe ES el mínimo**, así que el orden queda libre y el scan arranca por
  los ids altos, que es donde están los agentes recién registrados. En Base es
  la diferencia entre recorrer 42 batches y acertar en el primero. Con balance
  ≥ 2 sigue siendo ascendente, porque ahí el contrato del lookup (el id más
  bajo) sí depende del orden.

**Corrección — y esto es lo más importante del cambio**
- **`balanceOf > 0` sin match ya no es "no tiene agente".** Es una
  contradicción: el registry dice que esa dirección tiene tokens y el scan no
  pudo atribuir ninguno, o sea que **el rango estaba mal**. Antes eso era
  `Ok(None)`, y `Ok(None)` en `POST /register` **es permiso para mintear**:
  le entregaba una identidad duplicada a alguien que ya tenía una — el daño de
  INC-2026-07-21 por otra puerta. Ahora es `Err` → 503 `retryable`, con `warn!`.
- La escalera que se pasa del techo devuelve **error**, no un máximo truncado.
  La sonda vieja dejaba de duplicar en 1.000.000 y después buscaba
  binariamente dentro del rango que ya había abandonado: respondía un máximo muy
  por debajo del real y convertía en 404 a todo agente por encima.
- `multicall_owner_of` valida que la cantidad de resultados coincida con la de
  llamadas. Un array corto corre todo el mapeo id→resultado y devuelve el
  agente **equivocado**, no ninguno.

**Que no vuelva a pasar**
- `scripts/erc8004_registry_capabilities.py`: reporta contra la cadena viva qué
  contesta cada registry (`totalSupply`, `supportsInterface`, máximo, costo
  secuencial). Es lo que habría cerrado esto en 30 segundos. Un nodo
  inalcanzable se reporta como **sin veredicto** y da exit 1 — nunca como una
  capacidad.
- 26 tests nuevos que manejan la búsqueda entera contra un registry en memoria,
  o sea que **el camino que toma producción es el que ejercita la suite**.
- `/identity/{network}/total-supply` deja de ser un 501 muerto: responde
  `highestAgentId` derivado, con `source` y la aclaración de que no es la
  supply si hubo quemas.

## Lo que queda abierto

- **Base está en 83.984 agentes contra los 192.000 que el scan puede caminar**
  (cap subido de 64 a 96 batches en este cambio; antes eran 128.000 y ya se
  había consumido el 65%). Cuando lo cruce, **todas** las consultas de owner en
  Base pasan a 503. Ahora avisa por `warn!` desde el 75%. **La solución de
  fondo es un índice de owner, no un cap más grande** — y está bloqueada porque
  los registries no exponen owner→agentId, no son `ERC721Enumerable`, y SKALE
  limita `eth_getLogs` a 2000 bloques.
- `OWNER_LOOKUP_TTL` sigue en 300s y la caché es **por tarea**, en memoria, con
  3 tareas corriendo: el acierto tope es ~1/3. Una caché compartida bajaría más
  el p99, pero no hacía falta para cerrar esto.
- Falta verificar en producción. Criterio: `/identity/celo/owner/<addr>` en frío
  por debajo de 1s, y las dos alarmas en OK 30 minutos seguidos.

---

## Adenda 2.4.0 — el arreglo estaba a medias, y por mi culpa

2.2.0 salió a producción a las 14:10 UTC. Medido con 50 minutos de tráfico
orgánico (14:20-15:05, sin binario viejo y sin mis propios requests):

| | antes | 2.2.0 |
|---|---|---|
| p99 | 11,4s | **3,5-5,2s** |
| p90 | 4,7s | **0,4-1,7s** |
| p50 | 0,048s | 0,055s |

Mejora real, ~2,5x. **Pero `p99-early` volvió a ALARM**, y la causa fue algo que
introduje yo en el mismo commit.

### El defecto

`ScanOrder::AnyMatch` barría de ids altos a bajos, apoyado en "los agentes recién
registrados tienen id alto". Medí dos direcciones de Base que dieron 58.585 y
60.720 y generalicé.

La distribución real, de 3h de logs de producción:

| red | n | mediana | posición |
|---|---|---|---|
| celo | 569 | 9.732 de 9.802 | batch 5 de 5 — arriba |
| monad | 457 | 10.183 | batch 6 de 6 — arriba |
| ethereum | 361 | 46.997 | arriba |
| polygon/optimism/avalanche/arbitrum/skale | ~2.400 | 529-1.790 | 1-2 batches |
| **base** | **820** | **18.897 de 83.984** | **batch 10 de 42 — ABAJO** |

Acertaba en 8 de 9. Fallaba en la única con 42 batches y la de más tráfico: la
consulta típica de Base quedaba 33ª de 42, más de ocho waves. Por eso los lentos
que quedaron eran todos `/identity/base/owner/...`, entre 4 y 9,6s.

**Es la misma forma exacta del bug que este documento describe**: un supuesto
plausible sobre la cadena, nunca contrastado contra ella, que se ve correcto en
la muestra que uno mismo eligió. El original duró tres días; este, seis horas.

### El arreglo

`SCAN_HINT_CACHE`: el scan recuerda en qué agente encontró el último match de ese
registry y arranca por ese batch, expandiéndose hacia los dos lados. **No supone
nada: mide.** Si mañana EM registra agentes cerca de 84.000, el hint los sigue
solo.

Sin hint todavía aprendido, expansión bidireccional desde los dos extremos, para
que ningún extremo sea patológico. El peor caso pasa de "todo el registry" a "la
mitad".

La corrección no se toca: con `balanceOf == 1` cualquier match sigue siendo el
mínimo, así que el orden sólo cambia cuánto tarda la respuesta, nunca cuál es.

La propiedad que no puede romperse es que el orden sea una **permutación** de los
batches: un índice perdido no falla ruidosamente, se saltea una porción del
registry y responde "no registrado", que es lo que los llamadores persisten. El
test la verifica exhaustivamente para 1..45 batches contra cada posición de hint,
incluidas las de fuera de rango.

### Por qué no simplemente "ascendente"

Ascendente hoy sería mejor que descendente para Base. Pero es otro supuesto sobre
la distribución, correcto por casualidad: el día que EM registre agentes cerca del
techo, esos pasan a ser los lentos y volvemos acá. El hint no elige un extremo.

