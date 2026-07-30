#!/usr/bin/env python3
"""
Run every on-chain scan and merge the results.

The entry point a cheap model or a cron job runs without reasoning about
anything: no chain list to maintain, no decisions to make, one command.

    python scripts/scan/run_all.py

Reports land in ``docs/reports/onchain-scan/``. A scanner that fails does not
stop the others — its networks simply come back UNSCANNED, which the merged
report states explicitly. That distinction is the whole point: an unreachable
explorer must never be summarised as zero payments.
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
SCAN_DIR = REPO / "scripts" / "scan"
OUT_DIR = REPO / "docs" / "reports" / "onchain-scan"

#: Each scanner writes its own JSON. Adding a chain family means adding one
#: script and one line here.
SCANNERS = [
    ("EVM mainnets", [sys.executable, str(SCAN_DIR / "scan_evm.py")], "evm-mainnets.json"),
    ("EVM testnets", [sys.executable, str(SCAN_DIR / "scan_evm.py"), "--testnets"], "evm-testnets.json"),
]


def main() -> int:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    merged = {
        "generatedAt": int(time.time()),
        "source": "on-chain scan of facilitator wallets",
        "caveat": (
            "Counts x402 settlements identified by method selector. This is a "
            "DIFFERENT number from the facilitator's own records: this one covers "
            "all history but only sees what the chain shows, while the store covers "
            "only since it was enabled but sees every operation including verifies. "
            "Do not add them together."
        ),
        "scanners": {},
        "totalPayments": 0,
        "networksScanned": 0,
        "networksSkipped": 0,
    }

    failures = []
    for label, command, output in SCANNERS:
        print(f"\n=== {label} ===")
        result = subprocess.run(command, cwd=REPO)
        path = OUT_DIR / output
        if result.returncode != 0 or not path.exists():
            # A scanner that died leaves its networks unknown. Recording the
            # failure beats omitting the section and letting the merged total
            # read as complete.
            failures.append(label)
            merged["scanners"][label] = {"ok": False, "reason": f"exit {result.returncode}"}
            continue

        report = json.loads(path.read_text(encoding="utf-8"))
        merged["scanners"][label] = {
            "ok": True,
            "file": str(path.relative_to(REPO)),
            "payments": report["totalPayments"],
            "networksScanned": report["networksScanned"],
            "networksSkipped": report["networksSkipped"],
        }
        merged["totalPayments"] += report["totalPayments"]
        merged["networksScanned"] += report["networksScanned"]
        merged["networksSkipped"] += report["networksSkipped"]

    if failures:
        merged["incomplete"] = True
        merged["failedScanners"] = failures

    out = OUT_DIR / "merged.json"
    out.write_text(json.dumps(merged, indent=2), encoding="utf-8")

    print("\n" + "=" * 56)
    print(f"pagos x402 (histórico on-chain): {merged['totalPayments']}")
    print(f"redes escaneadas: {merged['networksScanned']}  ·  sin escanear: {merged['networksSkipped']}")
    if failures:
        print(f"INCOMPLETO — fallaron: {', '.join(failures)}")
    print(f"-> {out.relative_to(REPO)}")
    # Exit non-zero on partial results so a cron job or CI step notices rather
    # than filing an incomplete report as a success.
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
