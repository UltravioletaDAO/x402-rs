#!/usr/bin/env python3
"""parse_server_log.py -- percentiles del propio facilitador, por fase y por ruta.

Por que existe: el reloj del generador de carga incluye su propio event loop y el
scheduler de la maquina. En un laptop compartido eso mete p95 de segundos en rutas
de 0,4 ms, que es ruido del cliente y no del servidor.

`telemetry.rs` ya emite `status=NNN elapsed=Nms` por request, con la MISMA
resolucion que los logs de produccion. Leerlo de ahi da un numero comparable
uno a uno con la tabla de CloudWatch, sin el ruido del cliente.

Uso:
  python3 scripts/bench/parse_server_log.py --log out/facilitator.log \
      --bench out/bench_capacidad-2w.json
"""
import argparse
import json
import math
import re
from collections import defaultdict
from datetime import datetime

ANSI = re.compile(r"\x1b\[[0-9;]*m")
LINE = re.compile(
    r"^(?P<ts>\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+Z).*?"
    r"uri=(?P<uri>/\S*).*?status=(?P<st>\d+) elapsed=(?P<el>\d+)ms")


def pct(v, p):
    if not v:
        return None
    return v[min(len(v) - 1, int(math.ceil(p / 100 * len(v))) - 1)]


def route_of(uri):
    path = uri.split("?", 1)[0]
    parts = [p for p in path.split("/") if p]
    if not parts:
        return "/"
    if parts[0] in ("discovery", "dx402", "identity", "reputation", "feedback", "api",
                    "escrow", ".well-known") and len(parts) > 1:
        return "/" + parts[0] + "/" + parts[1]
    return "/" + parts[0]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--log", required=True)
    ap.add_argument("--bench", required=True, help="bench_<label>.json con las ventanas de fase")
    ap.add_argument("--out")
    a = ap.parse_args()

    bench = json.load(open(a.bench))
    windows = []
    for name, ph in bench["phases"].items():
        w = ph.get("window")
        if w:
            windows.append((name, w["start"], w["end"]))
    if not windows:
        raise SystemExit("el JSON del banco no trae ventanas de fase; correr run_bench.py actualizado")

    samples = defaultdict(lambda: defaultdict(list))
    status = defaultdict(lambda: defaultdict(lambda: defaultdict(int)))
    with open(a.log, errors="replace") as fh:
        for raw in fh:
            m = LINE.match(ANSI.sub("", raw))
            if not m:
                continue
            t = datetime.fromisoformat(m.group("ts").replace("Z", "+00:00")).timestamp()
            for name, s, e in windows:
                if s <= t <= e:
                    r = route_of(m.group("uri"))
                    samples[name][r].append(int(m.group("el")))
                    status[name][r][m.group("st")] += 1
                    break

    out = {}
    for phase, routes in samples.items():
        out[phase] = {}
        for r, v in routes.items():
            v.sort()
            out[phase][r] = {"n": len(v), "p50": pct(v, 50), "p95": pct(v, 95),
                             "p99": pct(v, 99), "max": v[-1],
                             "status": dict(status[phase][r])}
    txt = json.dumps(out, indent=1)
    if a.out:
        open(a.out, "w").write(txt)
    print(txt)


if __name__ == "__main__":
    main()
