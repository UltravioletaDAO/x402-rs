# Handoff — explorers muertos + stream `/events` · pendiente DEPLOY

**Para:** la sesión principal del facilitador (x402-rs)
**De:** una **sesión alterna** de Claude Code, trabajando desde el repo de
**KarmaCadabra** (`Z:\ultravioleta\dao\karmakadabra`) sobre el dashboard/observatorio.
**Fecha:** 2026-07-28
**Commit:** `ff3c4123` — *fix(explorers): 4 block explorers muertos*

> ## ✅ ESTA PARTE YA SE DESPLEGÓ — no hay que hacer nada
> Verificado 2026-07-28: `/version` responde **1.59.4** y el landing en vivo ya sirve
> `monadscan.com` y `skale-base-explorer.skalenodes.com`. Alguien de tu lado cortó
> `2ce559f5 chore(release): v1.59.4` y lo desplegó. Gracias.
>
> **LO ÚNICO PENDIENTE es la ADENDA del final** (`GET /events`, 4 commits sin pushear).

---

## Por qué te escribo

Soy una sesión paralela: estaba arreglando el observatorio de KarmaCadabra cuando el
operador (Saul) reportó que **los agentes publican links de block explorer que no
abren**. Al rastrearlo, el origen no era solo de KK: **`config/supported_tokens.json`
de este repo — la fuente de verdad JSON — tenía 4 explorers muertos**, y dos de ellos
además están **hardcodeados como links clickables en el landing público**
(`static/index.html`).

Lo corregí acá y lo commiteé, pero **no despliego el facilitador** (no es mi sesión ni
mi pipeline, y el facilitador es infra crítica gestionada aparte). Te dejo el contexto
completo para que lo despliegues cuando te sirva.

---

## Qué está roto (auditoría empírica, no teoría)

Probé **cada dominio** con `curl` + User-Agent de navegador, y los mainnets también con
una **tx/address real**:

| Cadena | Explorer viejo | Resultado | Reemplazo | Resultado |
|---|---|---|---|---|
| **monad** (143) | `monad.socialscan.io` | **429 siempre** | `monadscan.com` | **200** |
| **skale-base** (1187947933) | `skale-base.explorer.skalenodes.com` | **no resuelve** | `skale-base-explorer.skalenodes.com` | **200** |
| **hyperevm** (999) | `purrsec.com` | **404** (incluso `/address/<real>`) | `hyperevmscan.io` | **200** |
| **skale-base-sepolia** (testnet) | `base-sepolia-testnet.skalenodes.com` | **404** | `base-sepolia-testnet-explorer.skalenodes.com` | **200** |

Notas de la auditoría:

- `monadscan.com` y `skale-base-explorer.skalenodes.com` son además los **canónicos de
  chainlist** (`chainid.network/chains.json`) para 143 y 1187947933.
- `hyperevmscan.io` **ya se usaba en el propio `static/index.html`** — el JSON estaba
  desalineado con el HTML del mismo repo.
- **NO toqué** `testnet.purrsec.com` (404) porque `testnet.hyperevmscan.io` **no
  resuelve**: no cambio lo que no puedo verificar que mejore.
- **avalanche y bsc dan 403 a `curl`**, pero eso es el **bloqueo anti-bot de
  Cloudflare** de la familia etherscan — abren perfecto en navegador. **No están rotos,
  no los toqué.** (Si alguien "arregla" eso basándose en un curl, rompe lo que sirve.)

## Qué cambié (2 archivos, 6 líneas)

```
config/supported_tokens.json   4 explorers (monad, skale-base, hyperevm, skale testnet)
static/index.html              2 links onclick de la wallet del facilitador (monad, skale)
```

`git show ff3c4123` tiene el diff exacto. **Solo stageé esos 2 archivos** — en el
working tree había trabajo ajeno sin commitear (`docs/marketing/*`,
`.claude/skills/ship-tweet/`, `.claude/commands/ship.md`) que **dejé intacto**.

## Lo que necesito de ti: el DEPLOY

`static/index.html` se hornea en el binario vía `include_str!`, así que **el landing
público sigue sirviendo los links muertos hasta que se rebuildee y redespliegue el
facilitador** (Fargate, **us-east-2**).

Cuando lo despliegues, verificar:

1. En `facilitator.ultravioletadao.xyz`, los links de la wallet del facilitador para
   **Monad** y **SKALE** abren de verdad (antes: 429 y dominio inexistente).
2. `config/supported_tokens.json` servido/consumido ya no tiene `socialscan` ni
   `purrsec`.

No hace falta tocar DNS ni nada más — es solo el binario con el landing nuevo.

## Lado KarmaCadabra (contexto, no requiere acción tuya)

Apliqué el mismo fix del lado de KK y **ya está desplegado el dashboard**:

- `agents_sdk/networks.py` — es lo que los **agentes** usan para armar los links que
  publican en `#agents`. ⚠️ Los agentes seguirán publicando el link viejo de Monad
  **hasta que se rebuildee la imagen del SDK de KK** (pendiente del lado de KK, no tuyo).
- `dashboard/live/js/{agent,live}.js` — corregido y desplegado.
- Tests: `tests/sdk/test_networks.py` 30 passed.

## Aprendizaje para futuras sincronizaciones

El docstring de `agents_sdk/networks.py` (KK) dice que los explorers se sincronizan
**desde** `config/supported_tokens.json` de este repo. Esta vez la fuente de verdad era
la que estaba mal: **verificar el dominio antes de propagar**. Un explorer muerto no
falla ruidosamente — simplemente hace que una tx real parezca falsa, que es exactamente
como lo reportó el operador ("¿alucinaste las transacciones?").


---

# ADENDA — Stream de tráfico en tiempo real (`GET /events`, SSE)

**Commits:** `cfc35491` (plan) · `dc52bdc1` (bus) · `be9a0f38` (endpoint + hook)
**Plan completo:** `docs/plans/traffic-events-stream.md`
**Estado:** ✅ **DESPLEGADO en v1.59.5** por la sesión principal, con tres correcciones
sobre lo entregado (ver *Revisión* al final). `cargo test --lib` = **308 passed / 0 failed**.

## Qué se agregó

`GET /events` — Server-Sent Events, un mensaje por operación del facilitador, para que
el observatorio de KarmaCadabra pinte el tráfico en vivo sin raspar CloudWatch.

- `src/events.rs` — bus `tokio::sync::broadcast` + config por env (6 tests).
- `handlers::events_routes()` + `get_events` — router con estado propio, keepalive 15s.
- `post_settle` publica al final, **después** de que el settle resolvió.
- `main.rs` — bus desde env, montado y compartido por `Extension`.

## Invariante que NO se puede regresionar

**El camino del dinero nunca se bloquea por el stream.** El canal es lossy: sin
suscriptores (o con uno rezagado) el evento se descarta. `publish()` devuelve `()` y no
propaga error jamás — un settle no puede fallar ni frenarse porque alguien esté mirando.
Un suscriptor `Lagged` pierde mensajes y **sigue conectado** en vez de tirar la conexión.

## Config al desplegar (todo opcional, defaults seguros)

| Variable | Default | Notas |
|---|---|---|
| `X402_EVENTS_ENABLED` | `true` | `false` → `/events` da 404 y no se publica nada |
| `X402_EVENTS_SCOPE` | `all` | `allowlist` = solo payers de la lista |
| `X402_EVENTS_ALLOWLIST` | *(vacío)* | direcciones por coma — aquí irían las wallets de KK |
| `X402_EVENTS_DETAIL` | `full` | `minimal` = solo `{ts, kind, network, ok}` |
| `X402_EVENTS_BUFFER` | `256` | |
| `X402_EVENTS_MAX_SUBSCRIBERS` | `64` | Al tope, `/events` da 503 + `Retry-After` |

⚠️ **Decisión consciente de Saul:** el default es `full` (payer + tx + monto) y `all`.
Se le señaló que ~la mayoría del tráfico NO es de KarmaCadabra y que eso difunde en vivo
la actividad de pagos de otros clientes; lo asumió a propósito. Por eso el detalle y el
alcance son **variables**: se pueden bajar sin tocar código.

## Verificación post-deploy

```bash
curl -N https://facilitator.ultravioletadao.xyz/events     # debe emitir eventos reales
```
Contrastar el ritmo contra `[SETTLEMENT]` en CloudWatch (~12/min medidos el 2026-07-28).
Hoy ese endpoint responde **404** (aún no desplegado) — el dashboard de KK ya lo consume
y simplemente se queda callado hasta que exista.

## Lado KarmaCadabra: ya desplegado

El dashboard ya consume el stream (`EventSource`) y pinta cada evento como una **ola
ámbar** que cae sobre la placa de su cadena — visualmente distinta de las pepitas de KK,
porque ese tráfico es del riel, no de nuestros agentes. Degrada en silencio mientras el
endpoint no exista.

---

## Revisión de la sesión principal antes de desplegar (v1.59.5)

Tres cosas se corrigieron sobre lo entregado. Ninguna invalida el diseño — el invariante
del camino del dinero estaba bien planteado y se respetó.

1. **`network` viajaba mal formado.** El evento usaba `format!("{:?}")`, que imprime el
   nombre de la *variante* del enum, no el slug. Para `Base` coincide por casualidad
   (`base`), pero `SkaleBase` salía como `skalebase` y `BaseSepolia` como `basesepolia` —
   nombres que no existen en `/supported` ni corresponden a ninguna placa. Como
   `facilitatorWave()` hace `nodeIndex.get("chain:"+network)` y descarta lo que no
   reconoce ("unknown chain: no guessing"), esas ondas se habrían perdido **en silencio**,
   que es justo el modo de fallo que hace parecer que el stream "casi funciona". Ahora usa
   `Display`, el slug canónico que ya hablan `/supported` y el feed de trades.

2. **El rate limit y el tope de suscriptores no estaban.** El propio plan los lista como
   invariante **no negociable** (`## Invariantes de seguridad`), pero no se implementaron.
   `/events` es público, sin auth y de conexión larga, sobre la MISMA task que liquida
   pagos: `publish()` no puede frenarse por un observador, pero N observadores sí podían
   agotar el proceso sin tocar nunca el camino del dinero. Ahora hay governor
   (1 token/2s, burst 10, `SmartIpKeyExtractor` como el resto) y admisión tope 64
   (`X402_EVENTS_MAX_SUBSCRIBERS`) que responde 503 + `Retry-After` en vez de aceptar.

3. **Faltaban el hook de `verify` y el OpenAPI.** F1 dice "hook en `verify`/`settle`" y el
   dashboard ya registra `addEventListener("verify", …)`, pero solo `settle` publicaba: ese
   listener era código muerto. Un `verify` publica sin `tx` (no se liquidó nada todavía;
   inventar un hash haría mentir al stream). Y `CLAUDE.md` exige documentar todo endpoint
   nuevo en `src/openapi.rs` — `/events` ya sale en `/docs`.

**Sobre el default `full` + `all`:** se respetó la decisión de Saul, no se cambió. Queda
dicho una vez más para que no se pierda: el stream difunde payer, tx y monto de **todos**
los clientes del facilitador, no solo de KK. `X402_EVENTS_DETAIL=minimal` lo apaga sin
tocar código.

**F3 sigue abierto tal cual:** con `desired_count > 1` cada cliente pega a una instancia y
la ola sub-reporta. Hoy hay 1 task, así que no aplica.
