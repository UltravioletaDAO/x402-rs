# Backlog — failover de RPC por red

**Estado:** no implementado, y la decisión de no implementarlo fue explícita
(2026-08-03). No existe palanca de configuración.
**Por qué importa:** el RPC configurado de cada red es punto único de falla, y
el modo de fallo típico devuelve HTTP 200.

---

## Qué pasa hoy

`src/chain/evm.rs`, `FromEnvByNetworkBuild::from_env`:

```rust
let env_var = from_env::rpc_env_name_from_network(network);
let rpc_url = match std::env::var(env_var).ok() {
    Some(rpc_url) => rpc_url,
    None => { /* warn + skip */ return Ok(None); }
};
```

`from_env::rpc_env_name_from_network` devuelve un `&'static str` — **una** URL
por red. Lo mismo en `chain/solana.rs`, `chain/near.rs`, `chain/stellar.rs`,
`chain/xrpl.rs` y `chain/algorand.rs`. No hay lista, ni reintento contra un
segundo endpoint, ni health gating: si ese RPC se degrada, esa red queda muerta
hasta que alguien lo note y cambie la variable a mano.

La Lambda de balances **sí** tiene cadena de fallback
(`lambda/balances/handler.py`: privado -> env -> `PUBLIC_RPCS`). O sea que el
landing puede seguir mostrando saldos de una red cuyos settles llevan días
fallando. Eso ya pasó.

## Por qué se querría

El 2026-08-03 `rpc.celocolombia.org` llevaba días respondiendo HTTP 200 con el
chainId correcto (`0xa4ec`) y `eth_blockNumber` en `0x0`: el nodo se
resincronizaba desde cero y nunca terminó. Toda lectura de estado fallaba con
`-32801`, así que **todos los settles en Celo fallaban**, mientras cualquier
healthcheck ingenuo pasaba.

Un failover no habría evitado la degradación, pero la habría absorbido sin
intervención humana ni ventana de caída.

Y no es un caso aislado de Celo. De 22 endpoints medidos ese día, 9 eran
inservibles y de formas distintas: cadena equivocada, cabeza en el bloque 0,
10.8M bloques de atraso, 429 bajo ráfaga, y un balanceador comunitario que
fallaba 23/30 porque repartía sobre ese mismo pool roto. Cualquier red nuestra
puede caer en cualquiera de esas.

## Por qué NO se hizo

Decisión del usuario en la sesión del 2026-08-03, planteada explícitamente y
elegida sobre la alternativa: cambiar solo la URL, sin failover. El costo
asumido a cambio de no tocar Rust ni arriesgar el hot path de pagos.

Se deja escrito para que quede claro que es una deuda consciente, no un olvido.

## Por dónde empezaría

1. `rpc_env_name_from_network` devuelve la env var; el cambio arranca en hacer
   que su **valor** acepte lista separada por comas y que `from_env` construya N
   providers en vez de uno.
2. Calificar al arrancar, no solo al conectar. Los tres chequeos que separan un
   nodo sano de uno que miente son baratos: `eth_chainId` coincide,
   `eth_blockNumber != 0`, y el head no está más de N bloques por detrás del
   máximo observado entre los candidatos.
3. Failover en caliente: alloy permite envolver el transporte; lo mínimo útil es
   rotar al siguiente ante error de transporte o `-32801`/`-32003`.
4. **Ojo con el nonce.** Rotar de RPC en medio de la vía de escritura cruza con
   `PendingNonceManager`: dos nodos pueden discrepar en `getTransactionCount`
   `pending`. En la medición del 2026-08-03 los cinco finalistas coincidieron
   (326), pero eso no está garantizado. El failover de lectura es seguro; el de
   escritura necesita pensar el nonce antes.

## Cómo se comprueba que sirvió

Apuntar la lista de una red de testnet a `[endpoint-roto, endpoint-bueno]` en
ese orden y confirmar que `/supported` la sigue listando y que un settle pasa.
Hoy ese mismo escenario deja la red caída.

## Relacionado

- Skill `rpc-health` (`.claude/skills/rpc-health/`) — sondas, los siete modos de
  fallo que devuelven HTTP 200, y los cinco sitios donde vive `RPC_URL_*`.
- `references/failure-modes.md` — la evidencia de cada modo de fallo.
