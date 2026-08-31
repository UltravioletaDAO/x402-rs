# Zama Developer Program — investigación y veredicto de elegibilidad

**Fecha de la investigación:** 2026-08-31
**Estado:** research cerrado, aplicación redactada (ver `01-APPLICATION.md`)
**Método:** lectura de fuentes primarias de Zama + verificación en vivo contra producción

> Todo dato de esta página está marcado `[VERIFICADO]` con su fuente, o
> `[HIPÓTESIS]` cuando no se pudo confirmar. No inventar cifras aquí: es el
> insumo del texto que se manda a Zama.

---

## 1. Veredicto en una línea

**La "Mainnet Season 4" NO es la puerta para el facilitador.** Es un bounty
único y temático (PoolTogether confidencial en Sepolia) que no tiene relación
con infraestructura de pagos. La puerta correcta es el **Bounty Track general**
(explícitamente *tooling, documentation, integrations*), con el **Builder Track**
como segunda opción. Ambos son rolling — no dependen de la Season.

---

## 2. Qué es realmente la Season 4

`[VERIFICADO: https://www.zama.org/post/zama-developer-program-mainnet-season-4, leído 2026-08-31]`

Cita textual del anuncio: *"For this season, we're starting with one
production-oriented bounty that explores a practical application of confidential
assets."*

| Campo | Valor |
|---|---|
| Reto | Versión confidencial de PoolTogether: depósitos, balances y ganancias cifrados, con selección de ganador verificable on-chain |
| Deploy | **Sepolia** (textual: *"Deployments should target Sepolia"*) — pese al nombre "Mainnet Season 4" |
| Premio | 5.000 cUSDT, repartidos entre hasta 3 equipos; un proyecto excepcional puede llevarse todo |
| Extra | La mejor entrega puede recibir soporte de auditoría de OpenZeppelin y camino a producción |
| Inicio | 2026-07-30 |
| Deadline | 2026-09-05, 23:59 AOE |
| Form | `https://forms.zama.org/developer-program-mainnet-season4-bounty-track` |
| Contacto | developer@zama.org |
| KYC / restricción geográfica | No mencionados en el anuncio |

El anuncio agrega que *"additional bounty challenges may be introduced later
this quarter"* — o sea que pueden salir más retos antes de fin de Q3 2026.

**Por qué no aplicamos ahí:** un facilitador de pagos x402 con scheme FHE no es
una app de ahorro con premios. La entrega se evalúa contra un deliverable
concreto y específico; mandar infraestructura de pagos es off-topic y se
descarta sin leerlo.

---

## 3. Los tracks permanentes (la puerta real)

`[VERIFICADO: post de Season 1 en zama.org + https://community.zama.org/t/developer-program/4437]`

El Developer Program tiene cuatro tracks. Las Seasons son campañas encima de
esta estructura, no la reemplazan. El foro confirma que **se puede aplicar a
varios tracks** siempre que cada proyecto cumpla los requisitos del track.

| Track | Para qué | Reward | Encaje del facilitador |
|---|---|---|---|
| **Bounty Track** | *"tooling, documentation, integrations, and educational content that makes FHE more accessible"* | grants mensuales hasta $5k | **Encaje directo.** Un facilitador x402 que agrega `fhe-transfer` sobre ERC7984 es literalmente una integración de infraestructura |
| **Builder Track** | *"real-world use cases with the Zama Protocol"*, apps que empujan el límite técnico | grants mensuales hasta $5k | Encaje bueno: hay producto real en producción, no un prototipo |
| **Startup Track** | Proyectos early-stage con usuarios y potencial de revenue, más allá del prototipo de hackathon | rolling | Posible a futuro si el facilitador se monetiza |
| **Special Bounty Track** | Reto puntual (en Season 1 fue nómina confidencial on-chain) | $5k al mejor | No aplica hoy |

### Link de aplicación — PENDIENTE

`[HIPÓTESIS — falta confirmar]` No se pudo obtener la URL del formulario de
Builder/Bounty Track:

- `https://www.zama.org/developer-program` → **404**
- `https://www.zama.org/programs` → **404**
- El foro (`community.zama.org/t/developer-program/4437`) nombra los tracks pero no publica los links
- La única URL de formulario confirmada es la de la Season 4 bounty

**Acción requerida antes de mandar:** sacar el link del developer hub de Zama
(`zama.org/developer-hub`) o escribir a **developer@zama.org** pidiendo el
formulario del Bounty/Builder Track. **No inventar la URL.**

---

## 4. Estado del Zama Protocol en mainnet

`[VERIFICADO: post de Season 1 en zama.org + docs.zama.org/protocol]`

- El Zama Protocol **salió a mainnet en Ethereum el 30–31 de diciembre de 2025**, con la primera transferencia de stablecoin confidencial y staking en vivo.
- El token $ZAMA arrancó subasta el 12 de enero de 2026.
- **ERC7984 tiene configuración de FHE tanto para Ethereum mainnet como para Sepolia** (`docs.zama.org/protocol/examples/openzeppelin-confidential-contracts/erc7984`).
- Hay despliegues reales en mainnet hoy: USDC confidencial rindiendo vía Steakhouse USDC Prime en Morpho, y pagos de nómina en cUSDT.
- Roadmap: deploy con aceleración GPU en testnet (junio 2026), integración a mainnet en Q3 2026, con throughput esperado de cientos de tx/s por chain.

**Conclusión:** ir a mainnet es técnicamente posible hoy. No estamos esperando a
que Zama abra nada.

---

## 5. Estado real de NUESTRA integración FHE

Todo lo de esta sección fue verificado en vivo contra producción el 2026-08-31.

### Lo que funciona `[VERIFICADO]`

| Comprobación | Resultado |
|---|---|
| `/version` de producción | `{"version":"2.0.0"}` |
| `/supported` anuncia `fhe-transfer` | Sí — `ethereum-sepolia` (v1) y `eip155:11155111` (v2 CAIP-2) |
| Lambda `zama-facilitator.ultravioletadao.xyz/health` | `200`, `{"status":"ok","service":"x402-facilitator","version":"1.0.0","networks":["fhevm-local","sepolia"]}` |
| Forwarding end-to-end | **Confirmado.** Un `POST /verify` con `scheme: fhe-transfer` contra el facilitador principal devolvió la validación zod del Lambda — el request cruzó los tres saltos (ECS → proxy → Lambda) |

Ruta en el código:

- `src/types.rs:108` — `Scheme::FheTransfer`, serializado como `fhe-transfer`
- `src/handlers.rs:2440` — routing de `/verify` al Lambda, **antes** de deserializar el tipo (los payloads FHE tienen forma propia)
- `src/handlers.rs:3111` — el mismo routing para `/settle`
- `src/facilitator_local.rs:219-233` — las dos entradas de `/supported`; `extra: None` a propósito, el fee payer lo resuelve el Lambda
- `src/fhe_proxy.rs` — cliente HTTP, timeout 90s (la descifrada vía relayer de Zama es lenta y el Lambda tiene 60s + cold start)

### Lo que NO es cierto y no se puede afirmar en la aplicación `[VERIFICADO]`

- **Cero tráfico FHE real.** El único registro de `fhe-transfer` en
  `/transactions` es la sonda sintética que se lanzó durante esta investigación
  (ts `1788186723354`, `ok: false`). La ruta está viva pero nunca la usó nadie.
- **`FHE_FACILITATOR_URL` no existe en el terraform de producción.** El
  contenedor corre con el default hardcodeado en `src/fhe_proxy.rs:31`. Funciona,
  pero la URL del Lambda vive en un solo lugar del código — ver
  `02-MAINNET-READINESS.md`, punto 5.
- El Lambda solo declara `fhevm-local` y `sepolia`. **No conoce mainnet.**

### Contexto del facilitador que sí es citable `[VERIFICADO]`

| Dato | Valor | Fuente |
|---|---|---|
| Mainnets de pago servidas | **21** | `PYTHONUTF8=1 python scripts/verify_landing_canonical.py` |
| Stablecoins soportadas | **6** (USDC, USDT, EURC, AUSD, PYUSD, USDG) | `python scripts/stablecoin_matrix.py` |
| Familias de chain | **7** (EVM, Solana/SVM, NEAR, Stellar, Sui, Algorand, XRPL) | `src/network.rs:275` |
| Mainnets con escrow | 9 | `verify_landing_canonical.py` |
| Redes ERC-8004 | 12 mainnets / 21 total | `src/erc8004/mod.rs` |
| Operaciones de settle registradas OK | **2.306** (2.063 fallidas, 13 redes con actividad) | `GET /api/stats` |
| Licencia | Apache 2.0 | `LICENSE` |
| Repo | `github.com/UltravioletaDAO/x402-rs` (fork de `x402-rs/x402-rs`) | `git remote -v` |

> **Caveat obligatorio sobre las cifras de `/api/stats`:** ese índice **no es un
> ledger** — la fila se escribe fire-and-forget *después* de que la liquidación
> resuelve, así que un store inalcanzable pierde filas. Además, mientras
> `X402_EVENTS_PUBLISH_FAILURES` esté en `false`, las operaciones fallidas no
> generan fila. La cadena es la fuente de verdad. Si se cita un número a Zama,
> decirlo como "operaciones registradas", nunca como "volumen liquidado".

---

## 6. La tesis para Zama (por qué esto les sirve)

x402 es el estándar HTTP 402 para micropagos máquina-a-máquina: un servidor
responde `402 Payment Required` con lo que cobra, el cliente firma una
autorización, y un facilitador la verifica y la liquida. Es el carril de pago
que están adoptando los agentes de IA.

El problema estructural: **cada pago x402 publica en claro cuánto pagó quién a
quién**. Para un agente que consume cientos de APIs al día, eso es su
presupuesto, sus proveedores y su margen, todo público y correlacionable.

Zama resuelve exactamente eso, y ERC7984 es la primitiva. Lo que falta es que
alguien lo conecte al carril de pago que los agentes ya usan — no una demo, sino
un facilitador en producción con un scheme `fhe-transfer` al lado del `exact` de
siempre, para que un vendedor pueda cobrar confidencialmente cambiando un campo
del `402`.

Ese puente ya está construido y desplegado en Sepolia. Lo que le pedimos al
programa es llevarlo a mainnet.

---

## 7. Riesgos y objeciones esperables

| Objeción | Respuesta honesta |
|---|---|
| "No tienen uso real de FHE" | Cierto. La ruta está en producción pero sin tráfico. Es precisamente el gap que cierra el trabajo propuesto: mainnet + un integrador real. No pretender lo contrario. |
| "Esto es infraestructura, no una app" | Por eso se aplica al **Bounty Track** (*tooling, integrations*), no al de apps. |
| "El facilitador es un fork" | Es un fork de x402-rs (Apache 2.0) con ~20 redes agregadas por nosotros y varios subsistemas propios. Decirlo de frente; el trabajo FHE es 100% nuestro. |
| "¿Por qué no lo subieron a upstream?" | Está en el roadmap de la aplicación. La x402 Foundation exige PR revisado y descarta propuestas sin uso en producción — por eso primero mainnet, después upstream. |

---

## 8. Fuentes

- Season 4: https://www.zama.org/post/zama-developer-program-mainnet-season-4
- Season 4 en el foro: https://community.zama.org/t/zama-developer-program-mainnet-season-4-is-live/4630
- Estructura de tracks (Season 1): https://www.zama.org/post/zama-developer-program-mainnet-season1-building-for-the-long-game
- Categoría Developer Program del foro: https://community.zama.org/t/developer-program/4437
- ERC7984 en docs de Zama: https://docs.zama.org/protocol/examples/openzeppelin-confidential-contracts/erc7984
- Litepaper del protocolo: https://docs.zama.org/protocol/zama-protocol-litepaper
- Guild del Developer Program: https://guild.xyz/zama/developer-program

## 9. Documentos hermanos

- `01-APPLICATION.md` — el texto listo para mandar
- `02-MAINNET-READINESS.md` — el gap técnico de Sepolia a Ethereum mainnet
- `../../ZAMA_FHE_INTEGRATION.md` — cómo funciona la integración que ya existe
- `../../ZAMA_X402_INTEGRATION_PLAN.md` — el plan original (dic 2025)
