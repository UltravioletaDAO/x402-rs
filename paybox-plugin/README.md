# El plugin de PayBox del facilitador (el cascarón, 0.1.0)

Un plugin de PayBox de **un solo host**, `facilitator.ultravioletadao.xyz`, cuyas tools son **las lecturas
del MCP del facilitador**, con sus mismos nombres y sus mismas definiciones. Es un cascarón: no tiene lógica
de dominio, no firma, no paga, no liquida y no propone una transacción. Lo que hace cada tool lo hace el
facilitador por `POST /mcp`, que la corre por su propia ruta REST; el plugin la releva. Sigue el molde del
cascarón de Emporium (`emporium/paybox-plugin`): relevo con presupuesto de tiempo, sin lógica propia ni
firmas.

**En privado.** Este paquete no se sube a PayBox desde acá: c0der lo empaqueta y lo sube el dueño.

## Las tools

Cinco, todas de clase `lectura` (`_meta["uvd/clase"]`) y `readOnlyHint: true`:

| Tool | Lo que lee el facilitador |
|---|---|
| `x402_supported` | `GET /supported`: cada red y esquema que liquida |
| `x402_networks` | `GET /networks.json`: nombre, exploradores y tokens de cada red |
| `x402_chain_health` | `GET /health/ready`: si una cadena puede liquidar ahora (una lectura `down` es el resultado, no un error) |
| `x402_escrow_onchain_state` | `POST /escrow/state`: lo que un contrato de escrow guarda para un pago (una view call) |
| `x402_durable_evidence` | `GET /dx402/evidence/{paymentId}` o `GET /dx402/receipt/{paymentId}` |

**No** releva `x402_accepts`, `x402_verify` ni `x402_settle`: el plugin no las lista, y una llamada con
esos nombres es un error JSON-RPC sin que salga ningún request.

Cuatro de las cinco llegan con la 2.54.0 del facilitador. Contra una versión anterior, esas cuatro contestan
`unknown tool`: el plugin se sube después de que la 2.54.0 esté desplegada.

## Qué hay

| Archivo | Qué es |
|---|---|
| `src/catalogo.generated.ts` | El catálogo: las tools de clase `lectura` del `tools/list` del facilitador, tal cual. **Lo genera el facilitador** (abajo) y no se edita a mano |
| `src/catalogo.ts` | Lo lee y comprueba su forma al arrancar: una tool que no sea `lectura` y `readOnlyHint` no llega a `tools/list` |
| `src/server.ts` | El servidor MCP: `tools/list` es el catálogo; `tools/call` va al facilitador con el mismo nombre |
| `src/facilitator.ts` | El único lugar que usa la red: un `POST /mcp` por llamada, sin reintento ni redirección, con 6 s y 192 KiB como mucho |
| `src/instructions.ts` | Las `instructions` del servidor (menos de 1.000 caracteres, ASCII) |
| `src/plugin.ts` | La entrada que corre PayBox (la del starter 1.0.4) |
| `src/line-safe.ts`, `src/sdk-shims.ts` | Lo de los otros plugins de la casa: U+2028/U+2029 escapados en stdout, y el SDK sin AJV |
| `scripts/` | Las herramientas del starter 1.0.4, byte a byte, más `check-bundle.mjs` y `reproducible-build.sh` |
| `test/` | Las pruebas del cascarón (abajo); `test/fixtures/facilitator-tools.json` es el `tools/list` entero del facilitador, también generado |

Del starter queda afuera **`src/paybox.ts`** (los armadores de propuestas EVM, Solana y x402): este plugin
sólo lee.

## Regenerar el catálogo

Después de cambiar una tool en `src/mcp.rs` (su esquema, su texto, su clase), desde la raíz del repo:

```bash
X402_WRITE_PAYBOX_PLUGIN=1 cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera \
  --bin x402-rs -- mcp::tests::the_paybox_plugin_relays_exactly_the_reads_of_tools_list --exact
```

Escribe `src/catalogo.generated.ts` y `test/fixtures/facilitator-tools.json` desde el `tools/list` que
contesta el servidor, en proceso y sin red. Sin la variable, el mismo test los compara byte a byte: la
suite de Rust del CI se pone roja si el facilitador cambia una lectura y el plugin no. Una cadena de `0x`
+ 64 hex (la `salt` del ejemplo del escrow) se escribe con la `x` escapada, la misma cadena una vez
leída, para que el hook pre-commit no la tome por una llave.

## Probar y construir (Node 24.21.0, pnpm 11.11.0; en Windows, en Docker o WSL)

```bash
pnpm install --frozen-lockfile
pnpm test            # el del starter: tipos, dos builds iguales y la conformidad (muestra: x402_supported)
pnpm run test:unit   # las del cascarón
pnpm run check:bundle
bash scripts/reproducible-build.sh [commit] [carpeta]   # todo eso desde un export limpio, en node:24.21.0
```

Las pruebas no usan la red: `test/harness.ts` reemplaza `fetch` por uno que falla, y
`test/fake-mcp.test.ts` levanta un MCP falso en `127.0.0.1` que lista las ocho tools del facilitador.
**La conformidad sí hace una lectura real**: su llamada de muestra es `x402_supported` contra el
facilitador de producción, que no firma ni mueve nada. Lo que prueban: que `tools/list` es exactamente
las lecturas del `tools/list` del facilitador, definiciones incluidas, y nunca un riel de pago
(`catalog.test.ts`); que contra el MCP falso cada lectura llega una vez y vuelve igual, y que un riel nunca
llega (`fake-mcp.test.ts`); que cada llamada sale a un solo host y qué dice cuando no hay respuesta
(`forwarding.test.ts`); las `instructions` (`instructions.test.ts`); el nombre de la DAO y un solo host
(`brand.test.ts`); el starter byte a byte (`starter-integrity.test.ts`); y el bundle (`bundle.test.ts`).

El CI del repo corre tipos, dos builds iguales, las pruebas y el bundle en
`.github/workflows/paybox-plugin.yml`, sólo cuando cambia `paybox-plugin/`: no compila Rust ni despliega.

## En PayBox

Un solo host de red: `facilitator.ultravioletadao.xyz`. Sin variables ni secretos. El facilitador limita
`POST /mcp` por IP con el balde de `/verify` y `/settle` (una ficha cada 2 s, ráfaga de 30): todas las
llamadas que salgan de la misma IP de PayBox lo comparten, y un `429` se lee como «esperá un minuto».
