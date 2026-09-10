#!/usr/bin/env python3
"""run_bench.py -- banco de capacidad del facilitador contra un RPC simulado.

Que mide y por que asi:

* **Latencia por ruta** (p50/p95/p99) con la mezcla real de produccion, en modelo
  abierto. La mezcla sale de los logs de CloudWatch, no de una suposicion.
* **CPU por request**. En la Mac no hay cgroup de Fargate, asi que el numero que
  se traslada a una task no es "cuantos RPS aguanta este laptop" sino
  **milisegundos de CPU por request**: eso si escala con el tamano de la task
  (1 vCPU = 1000 ms de CPU por segundo de reloj). El anexo de la auditoria lo pide
  explicitamente en el paso 5.
* **RSS de pico**, que decide si 1 GiB alcanza.

Nada de esto toca produccion ni mainnet: el binario corre local y todos los
RPC_URL_* apuntan al mock en 127.0.0.1.

Uso:
  python3 scripts/bench/run_bench.py --binary target/release/x402-rs --out-dir out/
"""
import argparse
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import time
import urllib.request

import psutil

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))

# Todas las redes EVM del binario, apuntadas al mock en loopback. Se configuran
# todas y no solo la que recibe carga porque `/supported` enumera lo que quedo
# inicializado: con dos redes ese endpoint devuelve un cuerpo diez veces mas chico
# que el de produccion y la medicion no significaria nada.
#
# Ningun paquete sale de 127.0.0.1: son URLs de loopback, no endpoints de cadena.
# El trafico del banco solo usa base-sepolia (testnet).
BENCH_NETWORKS = {
    "RPC_URL_BASE_SEPOLIA": 84532, "RPC_URL_BASE": 8453,
    "RPC_URL_AVALANCHE_FUJI": 43113, "RPC_URL_AVALANCHE": 43114,
    "RPC_URL_POLYGON_AMOY": 80002, "RPC_URL_POLYGON": 137,
    "RPC_URL_OPTIMISM_SEPOLIA": 11155420, "RPC_URL_OPTIMISM": 10,
    "RPC_URL_CELO_SEPOLIA": 44787, "RPC_URL_CELO": 42220,
    "RPC_URL_ETHEREUM_SEPOLIA": 11155111, "RPC_URL_ETHEREUM": 1,
    "RPC_URL_ARBITRUM_SEPOLIA": 421614, "RPC_URL_ARBITRUM": 42161,
    "RPC_URL_UNICHAIN_SEPOLIA": 1301, "RPC_URL_UNICHAIN": 130,
    "RPC_URL_HYPEREVM_TESTNET": 333, "RPC_URL_HYPEREVM": 999,
    "RPC_URL_ROBINHOOD_TESTNET": 46630, "RPC_URL_ROBINHOOD": 4663,
    "RPC_URL_SKALE_BASE_SEPOLIA": 324705682, "RPC_URL_SKALE_BASE": 1187947933,
    "RPC_URL_MONAD": 143, "RPC_URL_BSC": 56, "RPC_URL_SCROLL": 534352,
    # XRPL/Stellar: sin llave arrancan en modo relay o se saltan, pero si el var
    # queda sin poner el binario se queda con el endpoint publico por defecto
    # (s1.ripple.com). Apuntarlos al mock garantiza que nada salga de la maquina.
    "RPC_URL_XRPL_MAINNET": 0, "RPC_URL_XRPL_TESTNET": 1,
    "RPC_URL_STELLAR": 0, "RPC_URL_STELLAR_TESTNET": 1,
}


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def wait_http(url, timeout=90):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2) as r:
                if r.status == 200:
                    return True
        except Exception:
            time.sleep(0.25)
    return False


class Facilitator:
    """Levanta el binario con un entorno de banco y mide su CPU/RSS."""

    def __init__(self, binary, port, rpc_base, log_path, extra_env=None):
        self.binary, self.port, self.rpc_base = binary, port, rpc_base
        self.log_path, self.extra_env = log_path, extra_env or {}
        self.proc = self.ps = None

    def env(self):
        e = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": os.environ.get("HOME", ""),
            "HOST": "127.0.0.1",
            "PORT": str(self.port),
            "RUST_LOG": os.environ.get("BENCH_RUST_LOG", "info"),
            "SIGNER_TYPE": "private-key",
            # Llave efimera generada por el orquestador. No es de nadie y no
            # tiene fondos en ninguna cadena.
            "EVM_PRIVATE_KEY": os.environ["BENCH_EVM_PRIVATE_KEY"],
            # Sin tablas DynamoDB: el store de transacciones, la idempotencia y
            # el nonce store quedan apagados, igual que en un arranque sin AWS.
            # El writer lease sin control plane deja IS_WRITER=true (ver
            # writer_lease.rs), que es lo que queremos: una sola task escritora.
            "ENABLE_WRITER_LEASE": "false",
            "AWS_EC2_METADATA_DISABLED": "true",
            "AWS_REGION": "us-east-2",
            # Los jobs de discovery se apagan: son trabajo de fondo por replica y
            # contaminarian la medida de CPU por request (hallazgo A4).
            "DISCOVERY_ENABLE_AGGREGATION": "false",
            "DISCOVERY_ENABLE_CRAWLER": "false",
            "DISCOVERY_ENABLE_HEALTH": "false",
            "ENABLE_ESCROW": "true",
            "ENABLE_UPTO": "true",
            "X402_EVENTS_ENABLED": "true",
            "X402_EVENTS_DETAIL": "full",
            "X402_EVENTS_SCOPE": "all",
            "X402_EVENTS_PUBLISH_FAILURES": "true",
            "TX_RECEIPT_TIMEOUT_SECS": os.environ.get("BENCH_RECEIPT_TIMEOUT", "20"),
            # Para que /version devuelva lo mismo que produccion y el cuerpo pese igual.
            "FACILITATOR_VERSION": open(os.path.join(ROOT, "VERSION")).read().strip(),
        }
        for var, chain_id in BENCH_NETWORKS.items():
            e[var] = f"{self.rpc_base}/evm/{chain_id}"
        e.update(self.extra_env)
        return e

    def start(self):
        log = open(self.log_path, "wb")
        self.proc = subprocess.Popen(
            [self.binary], cwd=ROOT, env=self.env(), stdout=log, stderr=subprocess.STDOUT
        )
        self.ps = psutil.Process(self.proc.pid)
        if not wait_http(f"http://127.0.0.1:{self.port}/health"):
            self.stop()
            raise RuntimeError(f"el facilitador no respondio /health; ver {self.log_path}")
        return self

    def sample(self):
        c = self.ps.cpu_times()
        try:
            rss = self.ps.memory_info().rss
        except Exception:
            rss = 0
        return {"cpu": c.user + c.system, "rss": rss, "t": time.time()}

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.proc.kill()


def run_phase(fac, name, mix_path, rps, duration, clients, out_dir, warmup=5):
    """Corre una fase de carga y devuelve latencias + CPU/RSS atribuibles a ella."""
    before = fac.sample()
    peak_rss = before["rss"]
    t_start = time.time()
    res_path = os.path.join(out_dir, f"load_{name}.json")
    proc = subprocess.Popen(
        ["node", os.path.join(HERE, "loadgen.js"),
         "--base", f"http://127.0.0.1:{fac.port}", "--mix", mix_path,
         "--rps", str(rps), "--duration", str(duration), "--clients", str(clients),
         "--warmup", str(warmup), "--out", res_path],
        stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT,
    )
    # Muestrea RSS mientras corre; el pico es el numero que decide el tamano.
    while proc.poll() is None:
        time.sleep(0.5)
        try:
            peak_rss = max(peak_rss, fac.ps.memory_info().rss)
        except Exception:
            pass
    after = fac.sample()
    with open(res_path) as fh:
        load = json.load(fh)
    cpu_s = after["cpu"] - before["cpu"]
    wall_s = after["t"] - before["t"]
    done = sum(r["done"] for r in load["routes"].values())
    # La ventana empieza despues del warmup (y del prewarm de conexiones del
    # generador) para que los percentiles del log del servidor cubran lo mismo
    # que los del cliente.
    load["window"] = {"start": t_start + warmup + 1.0, "end": after["t"]}
    load["resources"] = {
        "cpu_seconds": round(cpu_s, 3),
        "wall_seconds": round(wall_s, 3),
        "cpu_cores_busy": round(cpu_s / wall_s, 3) if wall_s else None,
        "requests_done": done,
        "cpu_ms_per_request": round(cpu_s * 1000 / done, 4) if done else None,
        "peak_rss_bytes": peak_rss,
        "peak_rss_mib": round(peak_rss / 1048576, 1),
    }
    with open(res_path, "w") as fh:
        json.dump(load, fh, indent=1)
    return load


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", default=os.path.join(ROOT, "target/release/x402-rs"))
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--duration", type=int, default=60)
    ap.add_argument("--clients", type=int, default=64)
    ap.add_argument("--rps", type=str, default="", help="lista de RPS separada por coma")
    ap.add_argument("--rpc-latency-ms", type=int, default=25,
                    help="latencia de cada respuesta del RPC simulado")
    ap.add_argument("--receipt-delay-ms", type=int, default=0,
                    help="ms hasta el primer recibo de una tx (0 = confirma al toque)")
    ap.add_argument("--worker-threads", type=int, default=0,
                    help="TOKIO_WORKER_THREADS; 0 = default del runtime")
    ap.add_argument("--label", default="run")
    ap.add_argument("--catalog", default="",
                    help="snapshot JSON del catalogo del Bazaar; vacio = catalogo sintetico")
    ap.add_argument("--catalog-size", type=int, default=39589,
                    help="recursos a sembrar cuando no hay --catalog")
    ap.add_argument("--per-route", action="store_true",
                    help="ademas de la mezcla, mide cada ruta sola a baja tasa "
                         "para atribuirle su CPU y su tiempo de servicio")
    ap.add_argument("--per-route-rps", type=int, default=8)
    ap.add_argument("--per-route-seconds", type=int, default=20)
    a = ap.parse_args()

    os.makedirs(a.out_dir, exist_ok=True)
    if not os.path.exists(a.binary):
        sys.exit(f"no existe el binario {a.binary}; compilar primero (ver README.md)")

    # El binario aborta si falta config/blacklist.json (esta en .gitignore; en
    # produccion lo pone el deploy). Sin esto el facilitador no arranca y el
    # error que se ve es "Failed to initialize compliance checker".
    bl = os.path.join(ROOT, "config/blacklist.json")
    if not os.path.exists(bl):
        shutil.copyfile(os.path.join(ROOT, "config/blacklist.json.example"), bl)

    # Llave efimera del facilitador para el banco.
    from eth_account import Account
    os.environ.setdefault("BENCH_EVM_PRIVATE_KEY", "0x" + Account.create().key.hex().replace("0x", ""))

    payload_dir = os.path.join(a.out_dir, "payloads")
    subprocess.run([sys.executable, os.path.join(HERE, "make_payloads.py"),
                    "--out-dir", payload_dir, "--count", "1"], check=True,
                   stdout=subprocess.DEVNULL)
    payload = os.path.join(payload_dir, "payment_000.json")

    rpc_port, fac_port = free_port(), free_port()
    rpc_env = dict(os.environ)
    rpc_env["MOCK_RPC_LATENCY_MS"] = str(a.rpc_latency_ms)
    rpc_env["MOCK_RPC_RECEIPT_DELAY_MS"] = str(a.receipt_delay_ms)
    rpc_log = open(os.path.join(a.out_dir, "mock_rpc.log"), "wb")
    rpc = subprocess.Popen(["node", os.path.join(HERE, "mock_rpc.js"), "--port", str(rpc_port)],
                           env=rpc_env, stdout=rpc_log, stderr=subprocess.STDOUT)
    time.sleep(0.7)

    extra = {}
    if a.worker_threads:
        extra["TOKIO_WORKER_THREADS"] = str(a.worker_threads)
    fac = Facilitator(a.binary, fac_port, f"http://127.0.0.1:{rpc_port}",
                      os.path.join(a.out_dir, "facilitator.log"), extra)

    import platform
    results = {"label": a.label, "started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
               "config": vars(a), "phases": {},
               "host": {"machine": platform.machine(), "system": platform.system(),
                        "release": platform.release(),
                        "cpus": psutil.cpu_count(logical=True),
                        "mem_gib": round(psutil.virtual_memory().total / 2**30, 1)}}
    try:
        t_start = time.time()
        fac.start()
        results["startup_seconds"] = round(time.time() - t_start, 2)
        results["startup"] = fac.sample()

        # Catalogo del Bazaar. /discovery/resources es el 37 % de la mezcla y con
        # el catalogo vacio contesta en 0 ms: sin esto el banco no mide la ruta.
        seed_cmd = [sys.executable, os.path.join(HERE, "seed_catalog.py"),
                    "--base", f"http://127.0.0.1:{fac_port}"]
        seed_cmd += (["--from-file", a.catalog] if a.catalog
                     else ["--synthetic", str(a.catalog_size)])
        seed = subprocess.run(seed_cmd, capture_output=True, text=True)
        results["catalog_seed"] = json.loads(seed.stdout or "{}") if seed.returncode == 0 else {
            "error": seed.stderr[-500:]}
        with urllib.request.urlopen(
                urllib.request.Request(f"http://127.0.0.1:{fac_port}/discovery/stats",
                                       headers={"x-forwarded-for": "10.250.0.1"}),
                timeout=30) as r:
            st = json.load(r)
        results["catalog"] = {"total": st.get("total"), "visible": st.get("visible")}

        # /accepts negocia una lista de requisitos; sin "accepts" contesta 400 sin
        # tocar nada. El cuerpo se arma con el mismo payload firmado.
        with open(payload) as fh:
            req = json.load(fh)["paymentRequirements"]
        accepts_path = os.path.join(a.out_dir, "accepts.json")
        with open(accepts_path, "w") as fh:
            json.dump({"x402Version": 1, "accepts": [req]}, fh)

        # Mezcla real: pesos = share de cada ruta en los logs de produccion de
        # los ultimos 7 dias, excluyendo /health (lo genera el propio ALB).
        mix = [
            {"name": "discovery_resources", "weight": 37.1, "method": "GET",
             "path": "/discovery/resources?limit=100"},
            {"name": "settle", "weight": 12.0, "method": "POST", "path": "/settle",
             "bodyFile": payload},
            {"name": "supported", "weight": 8.5, "method": "GET", "path": "/supported"},
            {"name": "landing", "weight": 7.1, "method": "GET", "path": "/"},
            {"name": "verify", "weight": 3.7, "method": "POST", "path": "/verify",
             "bodyFile": payload},
            {"name": "version", "weight": 2.1, "method": "GET", "path": "/version"},
            {"name": "blacklist", "weight": 1.9, "method": "GET", "path": "/blacklist"},
            {"name": "accepts", "weight": 1.8, "method": "POST", "path": "/accepts",
             "bodyFile": accepts_path},
            {"name": "health", "weight": 25.8, "method": "GET", "path": "/health"},
        ]
        mix_path = os.path.join(a.out_dir, "mix.json")
        with open(mix_path, "w") as fh:
            json.dump(mix, fh, indent=1)

        # Fase fria: primer contacto, sin warmup, corta. Mide el coste de la
        # primera vez (Lazy de network.rs, TLS/conexiones, catalogo).
        results["phases"]["cold"] = run_phase(
            fac, "cold", mix_path, rps=5, duration=15, clients=a.clients,
            out_dir=a.out_dir, warmup=0)

        # Linea de base en reposo: el proceso consume CPU sin trafico (jobs de
        # fondo, timers, el heartbeat de alloy). Sin restarla, cada ruta medida
        # sola carga con esa CPU y las baratas quedan sobrevaloradas.
        idle0 = fac.sample()
        time.sleep(20)
        idle1 = fac.sample()
        results["idle"] = {
            "seconds": round(idle1["t"] - idle0["t"], 2),
            "cpu_seconds": round(idle1["cpu"] - idle0["cpu"], 3),
            "cpu_cores": round((idle1["cpu"] - idle0["cpu"]) / (idle1["t"] - idle0["t"]), 5),
            "rss_mib": round(idle1["rss"] / 1048576, 1),
        }

        # Atribucion por ruta: cada una sola, a tasa baja y sin cola, para que
        # el tiempo de reloj sea tiempo de servicio y el CPU sea el de esa ruta.
        # Es lo unico que permite decir "esta ruta cuesta X" en vez de dar un
        # promedio de la mezcla, que es lo que la puerta A6 necesita.
        if a.per_route:
            for route in mix:
                one = os.path.join(a.out_dir, f"mix_{route['name']}.json")
                with open(one, "w") as fh:
                    json.dump([dict(route, weight=1.0)], fh)
                results["phases"][f"solo_{route['name']}"] = run_phase(
                    fac, f"solo_{route['name']}", one, rps=a.per_route_rps,
                    duration=a.per_route_seconds, clients=a.clients,
                    out_dir=a.out_dir, warmup=3)

        rps_list = [int(x) for x in a.rps.split(",")] if a.rps else [10, 50, 100, 200, 400]
        for rps in rps_list:
            results["phases"][f"warm_{rps}rps"] = run_phase(
                fac, f"warm_{rps}rps", mix_path, rps=rps, duration=a.duration,
                clients=a.clients, out_dir=a.out_dir, warmup=5)

        with urllib.request.urlopen(f"http://127.0.0.1:{rpc_port}/__stats", timeout=5) as r:
            results["mock_rpc_stats"] = json.load(r)
        results["final"] = fac.sample()
    finally:
        fac.stop()
        rpc.terminate()

    path = os.path.join(a.out_dir, f"bench_{a.label}.json")
    with open(path, "w") as fh:
        json.dump(results, fh, indent=1)
    print(path)


if __name__ == "__main__":
    main()
