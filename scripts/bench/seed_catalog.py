#!/usr/bin/env python3
"""seed_catalog.py -- carga el catalogo del Bazaar en un facilitador local.

Por que hace falta: `/discovery/resources` es el 37 % de la mezcla real y la ruta
de lectura mas cara (p50 53 ms en produccion). Con el catalogo vacio contesta en
0 ms y el banco mediria una ruta que no existe. En produccion el catalogo tiene
decenas de miles de recursos y vive en memoria en CADA task, asi que ademas es lo
que decide el RSS.

Dos fuentes:
  --from-file catalogo.json   snapshot real (el objeto de S3 que usa produccion)
  --synthetic N               N recursos generados, sin depender de AWS

El snapshot real se baja aparte, con credenciales de solo lectura:
  aws s3 cp s3://facilitator-discovery-prod/bazaar/resources.json /tmp/catalog.json --region us-east-2
No se commitea: son 100 MB de datos de terceros.

El limitador de /discovery/register es por IP (burst 250, 1 token cada 12 s), asi
que el seeder rota X-Forwarded-For sobre un pool grande. Con 400 IPs entran
100.000 recursos de una.
"""
import argparse
import json
import queue
import random
import string
import sys
import threading
import time
import urllib.error
import urllib.request

DEFAULT_ACCEPT = {
    "scheme": "exact",
    "network": "eip155:8453",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "amount": "10000",
    "payTo": "0x000000000000000000000000000000000000dEaD",
    "maxTimeoutSeconds": 60,
}


def to_register(res):
    """Traduce un item del catalogo al cuerpo de POST /discovery/register."""
    return {
        "url": res.get("url"),
        "type": res.get("type") or "http",
        "description": res.get("description") or "",
        "accepts": res.get("accepts") or [DEFAULT_ACCEPT],
        "metadata": res.get("metadata") or {},
    }


def synthetic(n):
    words = ["market", "weather", "vision", "audio", "index", "quote", "feed", "oracle"]
    for i in range(n):
        w = random.choice(words)
        yield {
            "url": f"https://bench-{i:06d}.example.com/{w}",
            "type": "http",
            "description": f"recurso sintetico {i} " + "".join(
                random.choice(string.ascii_lowercase + " ") for _ in range(120)),
            "accepts": [dict(DEFAULT_ACCEPT, amount=str(1000 + i % 50000))],
            "metadata": {"category": w, "provider": f"bench-{i % 97}", "tags": [w, "bench"]},
        }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:8080")
    ap.add_argument("--from-file")
    ap.add_argument("--synthetic", type=int, default=0)
    ap.add_argument("--limit", type=int, default=0, help="0 = todo")
    ap.add_argument("--threads", type=int, default=24)
    ap.add_argument("--ips", type=int, default=400)
    a = ap.parse_args()

    if a.from_file:
        with open(a.from_file) as fh:
            data = json.load(fh)
        items = data if isinstance(data, list) else next(
            v for v in data.values() if isinstance(v, list))
        items = [to_register(r) for r in items if r.get("url")]
    elif a.synthetic:
        items = list(synthetic(a.synthetic))
    else:
        sys.exit("hace falta --from-file o --synthetic N")
    if a.limit:
        items = items[: a.limit]

    q = queue.Queue()
    for i, it in enumerate(items):
        q.put((i, it))
    counts, lock = {}, threading.Lock()

    def worker():
        local = {}
        while True:
            try:
                i, body = q.get_nowait()
            except queue.Empty:
                break
            req = urllib.request.Request(
                a.base + "/discovery/register",
                data=json.dumps(body).encode(),
                headers={"content-type": "application/json",
                         "x-forwarded-for": f"10.{(i // a.ips) % 200 + 20}.{(i % a.ips) >> 8}.{(i % a.ips) & 255}"},
                method="POST")
            try:
                with urllib.request.urlopen(req, timeout=30) as r:
                    k = r.status
            except urllib.error.HTTPError as e:
                k = e.code
            except Exception as e:
                k = type(e).__name__
            local[k] = local.get(k, 0) + 1
        with lock:
            for k, v in local.items():
                counts[k] = counts.get(k, 0) + v

    t0 = time.time()
    threads = [threading.Thread(target=worker) for _ in range(a.threads)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    print(json.dumps({"submitted": len(items), "seconds": round(time.time() - t0, 1),
                      "status": {str(k): v for k, v in counts.items()}}))


if __name__ == "__main__":
    main()
