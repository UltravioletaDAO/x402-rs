#!/usr/bin/env python3
"""cost_model.py -- costo mensual de Fargate por opcion de tamano, con su margen.

Precios: los de la Price List de AWS para us-east-2, on-demand, Linux. Se pueden
volver a leer con

  aws pricing get-products --region us-east-1 --service-code AmazonECS \\
    --filters Type=TERM_MATCH,Field=usagetype,Value=USE2-Fargate-vCPU-Hours:perCPU \\
    --output json | jq -r '.PriceList[]|fromjson|.terms.OnDemand[].priceDimensions[].pricePerUnit.USD'

Capacidad: se calcula desde el CPU por request medido por el banco, no desde los
RPS que aguanto el laptop. 1 vCPU son 1000 ms de CPU por segundo de reloj, asi que

  RPS por task = (1000 * vCPU * utilizacion_objetivo - CPU_de_fondo) / cpu_ms_por_request

Uso:
  python3 scripts/bench/cost_model.py --cpu-ms-per-req 20.1 --peak-rps 33 --background-ms-per-s 5.6
"""
import argparse
import json

# USD por hora. Leidos de la Price List el 2026-09-10.
PRICES = {
    "x86": {"vcpu": 0.04048, "gib": 0.004445},
    "arm": {"vcpu": 0.03238, "gib": 0.003560},
}
HOURS = 730  # mes normalizado

# Combinaciones validas de Fargate que interesan aca. 1 vCPU con 1 GiB NO existe:
# para 1 vCPU el minimo es 2 GiB.
SIZES = {
    "0.25 vCPU / 0.5 GiB": (0.25, 0.5),
    "0.25 vCPU / 1 GiB": (0.25, 1),
    "0.5 vCPU / 1 GiB": (0.5, 1),
    "0.5 vCPU / 2 GiB": (0.5, 2),
    "1 vCPU / 2 GiB": (1, 2),
    "2 vCPU / 4 GiB": (2, 4),
}


def monthly(vcpu, gib, tasks, arch):
    p = PRICES[arch]
    return tasks * HOURS * (vcpu * p["vcpu"] + gib * p["gib"])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cpu-ms-per-req", type=float, required=True,
                    help="CPU por request de la mezcla real, medido por run_bench.py")
    ap.add_argument("--peak-rps", type=float, required=True,
                    help="pico agregado observado, en requests por segundo")
    ap.add_argument("--background-ms-per-s", type=float, default=0.0,
                    help="CPU de fondo por task, en ms de CPU por segundo de reloj")
    ap.add_argument("--target-util", type=float, default=0.60,
                    help="techo de CPU sostenida; la puerta de Astra pide menos de 60 por ciento")
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()

    rows = []
    for name, (vcpu, gib) in SIZES.items():
        budget = 1000 * vcpu * a.target_util - a.background_ms_per_s
        rps_task = budget / a.cpu_ms_per_req if budget > 0 else 0
        for tasks in (2, 3):
            for arch in ("x86", "arm"):
                cap = rps_task * tasks
                rows.append({
                    "size": name, "tasks": tasks, "arch": arch,
                    "usd_month": round(monthly(vcpu, gib, tasks, arch), 2),
                    "rps_per_task": round(rps_task, 1),
                    "rps_service": round(cap, 1),
                    "x_peak": round(cap / a.peak_rps, 2) if a.peak_rps else None,
                })
    if a.json:
        print(json.dumps({"prices": PRICES, "hours": HOURS, "input": vars(a), "rows": rows},
                         indent=1))
        return
    base = next(r for r in rows if r["size"] == "1 vCPU / 2 GiB" and r["tasks"] == 3
                and r["arch"] == "x86")
    print(f"CPU/request = {a.cpu_ms_per_req} ms ; fondo = {a.background_ms_per_s} ms/s por task ; "
          f"techo = {a.target_util:.0%} ; pico observado = {a.peak_rps} rps")
    print(f"\n{'opcion':22s} {'tasks':>5s} {'arq':>4s} {'USD/mes':>9s} {'vs hoy':>9s} "
          f"{'rps/task':>9s} {'rps servicio':>13s} {'x pico':>7s}")
    for r in sorted(rows, key=lambda r: (-r["usd_month"], r["size"])):
        d = r["usd_month"] - base["usd_month"]
        print(f"{r['size']:22s} {r['tasks']:5d} {r['arch']:>4s} {r['usd_month']:9.2f} "
              f"{d:+9.2f} {r['rps_per_task']:9.1f} {r['rps_service']:13.1f} {r['x_peak']:7.2f}")


if __name__ == "__main__":
    main()
