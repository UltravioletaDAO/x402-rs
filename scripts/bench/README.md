# Banco de capacidad del facilitador

Mide cuanto cuesta cada ruta del facilitador en CPU y en latencia, contra un RPC
simulado, para poder dimensionar la task de Fargate con numeros en vez de con
intuicion. Es el paso 5 de la secuencia de la auditoria de Astra 6.

Todo corre en local. **Nada apunta a produccion ni a mainnet**: los `RPC_URL_*`
del proceso bajo prueba se fijan a `127.0.0.1`, la red de los pagos es
`base-sepolia` y las firmas se generan con una llave efimera creada en el momento.

## Piezas

| Archivo | Que hace |
|---|---|
| `mock_rpc.js` | JSON-RPC EVM simulado, con latencia por metodo y demora de recibo configurables. Sin dependencias. |
| `make_payloads.py` | Firma EIP-712 `TransferWithAuthorization` de verdad, para que `/verify` y `/settle` lleguen al camino de pago. Necesita `eth_account`. |
| `seed_catalog.py` | Carga el catalogo del Bazaar en el store en memoria. Sin esto `/discovery/resources` contesta vacio y el banco no mide la ruta mas cara. |
| `loadgen.js` | Generador de carga de modelo abierto con mezcla ponderada y rotacion de `X-Forwarded-For`. Sin dependencias. |
| `run_bench.py` | Orquesta todo y escribe el JSON de resultados. Necesita `psutil` y `eth_account`. |

## Por que no k6 / oha / vegeta

Todas las rutas calientes llevan limitador por IP (`tower_governor` con
`SmartIpKeyExtractor`). Un generador que manda todo desde una IP mide el
limitador y no el servicio: `/verify` corta en burst 30 y despues 1 request cada
2 s. `loadgen.js` rota `X-Forwarded-For` sobre un pool de IPs sinteticas, que es
lo que hace el trafico real. Con `--clients 1` se reproduce el otro caso.

El modelo es abierto (llegadas a tasa fija) y no cerrado (N usuarios en bucle):
en modelo cerrado la tasa baja sola cuando el servidor se pone lento y los
percentiles salen mejores de lo que son. `scheduleLagMax` en la salida dice si el
generador llego a quedarse atras.

## Correr

```bash
# 1. Compilar el binario con las features de produccion (~11 min en frio).
cargo build --release --locked --features solana,near,stellar,algorand,sui,xrpl

# 2. Opcional: snapshot real del catalogo del Bazaar (credenciales de solo lectura).
#    Son ~100 MB de datos de terceros: no se commitea.
aws s3 cp s3://facilitator-discovery-prod/bazaar/resources.json /tmp/catalog.json --region us-east-2

# 3. Correr el banco.
python3 scripts/bench/run_bench.py \
  --out-dir /tmp/bench-2w --label capacidad-2w \
  --catalog /tmp/catalog.json \
  --duration 45 --rps 5,11,22,33,44,66 --clients 96 \
  --worker-threads 2 --per-route \
  --rpc-latency-ms 25 --receipt-delay-ms 0
```

Sin `--catalog` siembra un catalogo sintetico del mismo tamano
(`--catalog-size`, default 39589), y el banco deja de depender de AWS.

`--worker-threads` fija `TOKIO_WORKER_THREADS`. Produccion arranca con
`available_parallelism=2` en una task de 1 vCPU, asi que `2` es el equivalente;
`1` sirve como aproximacion de una task de 0.5 vCPU.

`--receipt-delay-ms` separa las dos preguntas: con `0` se mide capacidad de CPU y
pooling; con un valor tipo `6000` se mide como se comporta `/settle` mientras
espera la confirmacion de la cadena, que es lo que domina su latencia real.

## Que sale

`bench_<label>.json` con, por fase:

* `routes.<ruta>.p50/p90/p95/p99` y la distribucion de status.
* `resources.cpu_ms_per_request` -- **el numero que se traslada a Fargate**. En la
  Mac no hay cgroup, asi que "cuantos RPS aguanta este laptop" no significa nada;
  milisegundos de CPU por request si, porque 1 vCPU son 1000 ms de CPU por
  segundo de reloj.
* `resources.peak_rss_mib` -- indicativo. El RSS de macOS no se traslada a Linux;
  para dimensionar memoria vale el `MemoryUtilized` de CloudWatch.
* `idle` -- CPU en reposo, para restarla de las rutas medidas solas.

## Requisitos

`node` (sin paquetes), `python3` con `psutil`, `eth-account` y `eth-utils`.
`config/blacklist.json` lo crea el orquestador desde el `.example` si falta: el
binario aborta sin ese archivo.
