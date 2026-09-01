---
date: 2026-09-01
tags:
  - type/backlog
  - domain/evm
  - priority/p1
status: ready-to-build
urgency: 6/10
---

# El contador de nonce se adelanta y no sabe volver

**En una frase:** el contador de transacciones del facilitador se adelanta y no
sabe volver, así que los pagos con escrow en una red se traban hasta que alguien
reinicie — y el reinicio sirve diez minutos.

## Por qué 6 y no más ni menos

Medido el 2026-09-01 entre 22:00 y 23:00 UTC:

| | |
|---|---|
| `nonce too high` | 181 (reintentos, no 181 pagos distintos) |
| `transfer amount exceeds balance` (el disparador) | 24 |
| respuestas 200 del facilitador | 2.044 |

**No es un 9.** No se pierde dinero ni evidencia. El facilitador está sano: una
ruta (`/settle` → `settle_escrow` → `execute_authorize`) sobre una wallet EVM,
no una caída.

**No es un 3.** Está ocurriendo ahora, **se auto-perpetúa** —cada intento
fallido empeora el contador— y reiniciar sólo compra diez minutos. Comprobado
dos veces el mismo día: un restart a las 22:08 y un deploy a las 22:40, ambos
sanaron unos minutos y volvieron. Sin arreglo, cada racha de fallos se vuelve
permanente hasta el próximo deploy.

## El mecanismo, verificado en el código

`resync_target` (`src/chain/evm.rs:2398`) tiene dos salidas:

```rust
match (high_water, last_allocated) {
    // sana: confía en la cadena
    (_, Some(last)) if last.elapsed() >= NONCE_TRUST_CHAIN_AFTER => pending,
    // trinquete
    (Some(high_water), _) if pending <= high_water => high_water.saturating_add(1),
    _ => pending,
}
```

La salida sana exige `NONCE_TRUST_CHAIN_AFTER` = **120 segundos sin asignar
nonces** (`:2388`). Con fallos continuos, `last_allocated` se refresca en cada
intento, así que ese reloj **nunca llega**. Siempre cae en la segunda rama:
`high_water + 1` — y `high_water` acaba de subir por el intento que falló.

Secuencia real de los logs:

```
22:59:39   tx 1556   cadena 1555    brecha de 1, recién reiniciado
22:59:40   tx 1587   cadena 1555    +31 en UN segundo
23:00:44   tx 1603   cadena 1555    brecha de 48
```

El salto de 31 en un segundo es lo que muestra por qué escala: son intentos
concurrentes, cada uno pide un nonce y cada uno sube el `high_water` al que el
siguiente se ancla.

## La raíz, y por qué el arreglo es acotado

**`resync_target` no recibe el motivo por el que se está resincronizando.** Toma
`pending`, `high_water` y `last_allocated`, nada más — verificado en los dos
sitios de llamada (`:2465` y `:2693`).

Eso es todo el bug. La rama del trinquete protege un caso real —una transacción
nuestra propagándose, que el nodo aún no vio— pero al no saber el motivo aplica
esa protección también a `nonce too high`, que es **la señal opuesta**: prueba
positiva de que la cadena está *detrás* de nuestro contador, o sea que el
equivocado es `high_water`.

Los dos casos son distinguibles por el error y nunca se confunden:

| error | significa | qué hacer |
|---|---|---|
| `nonce too high` | la cadena va atrás nuestro | **confiar en la cadena** |
| `nonce too low` / `replacement underpriced` | nuestra tx está en vuelo | trinquetear (lo actual) |

El comentario del código ya dice que "el resync es la recuperación correcta"
para `nonce too high`. La intención está bien; la implementación del resync no la
cumple.

## El arreglo propuesto

Pasarle a `resync_target` el motivo, y que `nonce too high` tome la rama que
confía en la cadena, sin importar el reloj de 120 segundos.

No toca `NONCE_TRUST_CHAIN_AFTER` ni la protección contra `replacement
underpriced`, que sigue siendo correcta para el caso que la motivó.

## El disparador sigue SIN identificar

Este backlog decía primero que los settles sin fondos arrancaban la racha.
**Es falso**, y se comprobó midiendo por red en dos horas de logs:

| | red | consume nonce |
|---|---|---|
| 182 `nonce too high` | **arbitrum** | — |
| 6 `transfer amount exceeds balance` | **base** | **no** |

Distinta red, y las seis dicen textual `Gas estimation reverted; no nonce
consumed`: revierten en la estimación de gas, **antes de transmitir**, así que
no pueden mover el contador de nadie. Son dos problemas independientes que
coincidieron en el tiempo, y la coincidencia fue suficiente para que los dos los
leyéramos como causa y efecto.

Se anota porque el error tiene forma de repetirse: dos síntomas en la misma
ventana, los dos con "no hay fondos" en algún lugar de la historia, en dos redes
distintas que nadie miró hasta buscarlas.

**Ninguno de estos conteos es un total confiable, y conviene saberlo antes de
citarlos.** El campo `network=` no aparece en todas las líneas -- una consulta
devolvió 612 eventos sin un solo `network=` -- y el CLI de AWS pagina, así que
un `grep | sort | uniq -c` cuenta las líneas que SÍ lo traen, de la primera
página. Dos mediciones del mismo intervalo dieron 258 y 416 en arbitrum, y una
de las dos vio 9 en ethereum que la otra no. **Ninguna está mal: miden cosas
distintas y las dos parecen totales.**

Lo que sí se sostiene es la proporción, que es lo que decide: arbitrum domina
por dos órdenes de magnitud, y las de `transfer amount exceeds balance` están
todas en base con `no nonce consumed`. Para un conteo real hay que paginar hasta
el final y no asumir que el campo está siempre.

**Qué arranca la racha de Arbitrum sigue abierto.** El arreglo del trinquete
vale igual — evita que CUALQUIER racha se vuelva permanente, venga de donde
venga — pero no cierra esta pregunta.

## Los settles sin fondos, que sí son un problema propio

`0xd5791860ca10a6f39749fb499931c79c7c35071a` tiene **0,0178 USDC en Base** e
intenta settles por más de lo que tiene. 45 eventos en seis horas.

Es un agente de **Execution Market**, no tráfico externo: está hardcodeado como
`agent_id` en `mcp_server/tests/test_201_that_lies_cluster.py`, y es el mismo
`0xD5791860` que aparece en `docs/handoffs/2026-09-01-p99-identity-owner-lookup.md`.

Se arregla fondeando la wallet. No toca el trinquete.

## Lo que NO sirve

**Reiniciar.** El estado del nonce vive en memoria, así que todo deploy lo
limpia y el sistema parece sano unos minutos. Eso hace que un chequeo a los dos
minutos de un deploy dé un falso verde — pasó dos veces el 2026-09-01.

## Criterio de listo

- Un test que falle sin el arreglo: con `nonce too high` y `last_allocated`
  reciente, `resync_target` devuelve `pending` y no `high_water + 1`.
- La protección de `replacement underpriced` sigue verde.
- En producción: cero `nonce too high` sostenidos **una hora después** del
  deploy, no dos minutos — el reinicio enmascara el bug justo en esa ventana.

## Procedencia

Diagnosticado por la sesión de latencia p99 el 2026-09-01, verificado contra el
código por la sesión de DX402. El diff `78ccf934..origin/main` (2.5.0 → 2.7.0)
no toca `resync_target`, `high_water`, `NONCE_TRUST_CHAIN_AFTER`,
`is_nonce_error` ni `is_pre_broadcast_rejection`: ninguno de los deploys de ese
día lo arregló ni lo causó.

---

# El plan, listo para aplicar

Verificado contra `origin/main` a `e9dfe3b9`. **No aplicado**: esto es el plan,
no el cambio.

## Por qué hace falta pasar un dato nuevo

`resync_target` se llama desde `get_next_nonce` (`:2465`), cuando `state.next`
es `None`. Ahí no hay ningún error a mano — el error ocurrió antes, en el
reintento que llamó a `reset_nonce` (`:2520`). El único otro sitio de llamada
(`:2693`) es un helper de tests.

O sea que el motivo hay que **recordarlo en el reset y leerlo en el resync**. No
alcanza con cambiar la firma de `resync_target`: nadie en su sitio de llamada
sabe por qué está resincronizando.

## Los cambios

### 1. `NonceState` recuerda por qué se reseteó (`:2419`)

```rust
/// Por qué se invalidó el nonce cacheado, cuando se invalidó.
///
/// `resync_target` necesita distinguir dos señales opuestas que hoy trata
/// igual. Vive acá y no en la firma de `resync_target` porque el error ocurre
/// en `reset_nonce` y el resync pasa después, en otra llamada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonceResetReason {
    /// `nonce too high`: la cadena va DETRÁS de nuestro contador. Prueba
    /// positiva de que `high_water` es lo que está mal.
    ChainBehind,
    /// `nonce too low`, `replacement underpriced`, `already known`: una
    /// transacción nuestra puede seguir propagándose.
    MayBeInFlight,
}
```

Y el campo:

```rust
    /// Por qué se pidió el resync pendiente. `None` cuando no hay uno.
    reset_reason: Option<NonceResetReason>,
```

### 2. Un clasificador al lado de `is_nonce_error` (`:895`)

```rust
/// Cuál de las dos señales de nonce es este error.
///
/// `is_nonce_error` responde "¿reintento?"; esto responde "¿en quién confío?",
/// y son preguntas distintas. Sólo `too high` prueba que la cadena va atrás:
/// una transacción nuestra propagándose da `too low`, `already known` o
/// `replacement underpriced`, jamás `too high`.
fn nonce_reset_reason(error: &str) -> NonceResetReason {
    let lower = error.to_lowercase();
    if lower.contains("nonce") && lower.contains("too high") {
        NonceResetReason::ChainBehind
    } else {
        NonceResetReason::MayBeInFlight
    }
}
```

`MayBeInFlight` es el default a propósito: un error desconocido tiene que caer
en la rama conservadora, la que hoy ya existe.

### 3. `reset_nonce` toma el motivo (`:2520`)

Cambia a `reset_nonce(&self, address: Address, reason: NonceResetReason)` y
guarda `state.reset_reason = Some(reason);` junto al `state.next = None`.

Los llamadores del path de reintento pasan `nonce_reset_reason(&error_string)`
— el mismo string que ya le dan a `is_nonce_error`.

### 4. `resync_target` recibe el motivo (`:2398`)

```rust
fn resync_target(
    pending: u64,
    high_water: Option<u64>,
    last_allocated: Option<std::time::Instant>,
    reason: Option<NonceResetReason>,
) -> u64 {
    match (high_water, last_allocated) {
        // `nonce too high` es prueba de que la cadena va detrás nuestro, así
        // que el equivocado es `high_water`. Trinquetear acá es lo que
        // convierte una racha de fallos en una brecha permanente: cada intento
        // sube `high_water` y el siguiente se ancla ahí.
        _ if reason == Some(NonceResetReason::ChainBehind) => pending,
        // Nada de lo que asignamos puede seguir en vuelo: confiar en la cadena.
        (_, Some(last)) if last.elapsed() >= NONCE_TRUST_CHAIN_AFTER => pending,
        // Una transacción nuestra puede estar propagándose y este nodo no
        // haberla visto. Devolver un nonce <= high_water intentaría REEMPLAZARLA
        // en vez de encolarse detrás: el "replacement transaction underpriced"
        // que motivó esta rama.
        (Some(high_water), _) if pending <= high_water => high_water.saturating_add(1),
        _ => pending,
    }
}
```

El sitio de llamada (`:2465`) pasa `state.reset_reason`, y **limpia el motivo al
asignar**: `state.reset_reason = None;` junto a `state.next = Some(...)`. Sin
eso, un `ChainBehind` viejo desarmaría la protección de todos los resyncs
futuros.

`NONCE_TRUST_CHAIN_AFTER` no se toca. La protección de `replacement
underpriced` tampoco.

## Los tests

Cuatro, en el módulo que ya existe con `resync_nonce` (`:2688`), que hay que
extender con el parámetro nuevo.

```rust
#[test]
fn nonce_too_high_trusts_the_chain_even_with_a_fresh_allocation() {
    // El bug: 181 `nonce too high` en una hora, Arbitrum clavado en 1555
    // mientras nuestro contador llegaba a 1603. `last_allocated` se refresca
    // en cada intento fallido, así que la rama de los 120 segundos no llega
    // NUNCA y siempre gana `high_water + 1` -- con `high_water` recién subido
    // por el intento que acaba de fallar.
    let just_now = Some(std::time::Instant::now());
    assert_eq!(
        resync_nonce(1555, Some(1602), just_now, Some(NonceResetReason::ChainBehind)),
        1555,
        "la cadena va detrás nuestro: es `high_water` lo que está mal"
    );
}

#[test]
fn an_in_flight_transaction_still_gets_the_ratchet() {
    // La rama del trinquete protege un caso real y sigue intacta: una
    // transacción nuestra propagándose que este nodo no vio. Devolver un
    // nonce <= high_water la reemplazaría en vez de encolarse detrás.
    let just_now = Some(std::time::Instant::now());
    assert_eq!(
        resync_nonce(1555, Some(1602), just_now, Some(NonceResetReason::MayBeInFlight)),
        1603
    );
}

#[test]
fn an_unknown_error_takes_the_conservative_branch() {
    // Un error que no sabemos clasificar no puede desarmar la protección.
    assert_eq!(nonce_reset_reason("something we have never seen"), NonceResetReason::MayBeInFlight);
    assert_eq!(nonce_reset_reason("replacement transaction underpriced"), NonceResetReason::MayBeInFlight);
    assert_eq!(nonce_reset_reason("nonce too low"), NonceResetReason::MayBeInFlight);
    assert_eq!(
        nonce_reset_reason("nonce too high: address 0x103040..., tx: 1603 state: 1555"),
        NonceResetReason::ChainBehind
    );
}

#[test]
fn the_ratchet_does_not_escalate_across_concurrent_failures() {
    // El salto medido de +31 en un segundo: intentos concurrentes, cada uno
    // pide un nonce, cada uno sube el `high_water` al que el siguiente se
    // ancla. Con `ChainBehind` el resultado es estable, no acumulativo.
    let just_now = Some(std::time::Instant::now());
    let mut high_water = 1555u64;
    for _ in 0..30 {
        let n = resync_nonce(1555, Some(high_water), just_now, Some(NonceResetReason::ChainBehind));
        assert_eq!(n, 1555, "cada fallo debe devolver al mismo lugar, no correrse");
        high_water = high_water.max(n);
    }
}
```

**Cada uno tiene que fallar sin su arreglo.** Verificarlo reintroduciendo el
defecto, como se hizo con la escalera de autoridad de DX402 — un test que pasa
antes y después no prueba nada.

## Riesgo

Bajo y acotado. El cambio agrega una rama **antes** de las dos que ya existen y
no toca ninguna: con `MayBeInFlight` —el default para todo error desconocido—
el comportamiento es idéntico al de hoy, línea por línea.

El riesgo real no está en el código sino en la verificación: **reiniciar limpia
el estado del nonce**, así que un chequeo a los dos minutos del deploy da un
falso verde. Hay que mirar a la hora.
