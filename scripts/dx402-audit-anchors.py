#!/usr/bin/env python3
"""Audit every DX402 anchor for a pointer that names nothing, and repair it.

WHY THIS EXISTS

Until v2.2.0 the facilitator predicted an evidence pointer BEFORE uploading,
signed an EIP-712 receipt over the prediction, recorded it, and then discarded
the pointer the upload actually returned. Production runs the `ipfs` backend,
which is Pinata with S3 behind it, so a single Pinata failure -- a 10s timeout,
an expired JWT, any 5xx -- left the bytes safely in S3 while the record and the
signed receipt both named an IPFS object that never existed.

Reading that evidence fails silently by design: the fallback store treats the
primary's `NotFound` as a verdict and never retries the other half, and even if
it did, the S3 pointer parser rejects an `ipfs+` pointer as foreign. So the
anchor returned 201, the receipt carries the facilitator's signature, and the
evidence is unreachable forever with no error anywhere.

Nobody knows how many of the existing anchors are affected. That number is what
this script is for.

WHAT IT DOES

Scans the evidence registry, asks the facilitator to audit each anchor, and
reports one of three verdicts per record:

    healthy   the pointer resolves
    repaired  the pointer named nothing, the bytes were found, record corrected
    lost      the pointer named nothing and no store holds the bytes

Without --repair the facilitator audits and writes nothing, reporting
`repairable` for records it could fix. With --repair those are corrected --
which means the facilitator RE-SIGNS the receipt, since `pointer` is part of the
EIP-712 type hash. That is why the repair lives behind an endpoint rather than
in this script: the signing key must not leave the service.

Auditing is safe and rewriting a signed attestation is not, so the dangerous
half has to be asked for by name, on both sides.

USAGE

    export DX402_ADMIN_TOKEN=...            # same value as the facilitator's
    python scripts/dx402-audit-anchors.py                    # report only
    python scripts/dx402-audit-anchors.py --repair           # report and fix
    python scripts/dx402-audit-anchors.py --limit 20         # try a few first

Environment:
    DX402_ADMIN_TOKEN   required; gates POST /dx402/repair (404 without it)
    DX402_FACILITATOR   default https://facilitator.ultravioletadao.xyz
    DX402_TABLE         default facilitator-dx402-evidence
    AWS_REGION          default us-east-2
"""

import argparse
import json
import os
import sys
import urllib.error
import urllib.request

FACILITATOR = os.environ.get(
    "DX402_FACILITATOR", "https://facilitator.ultravioletadao.xyz"
).rstrip("/")
TABLE = os.environ.get("DX402_TABLE", "facilitator-dx402-evidence")
REGION = os.environ.get("AWS_REGION", "us-east-2")
TIMEOUT = 30


def scan_payment_ids(limit=None):
    """Every anchored paymentId, from the registry itself.

    There is no list endpoint -- `/dx402/stats` returns a count and nothing
    else -- so enumeration has to come from the table. `Scan` is granted to the
    task role and is the only read wide enough to answer "which anchors exist".
    """
    import boto3

    table = boto3.resource("dynamodb", region_name=REGION).Table(TABLE)
    ids, kwargs = [], {"ProjectionExpression": "payment_id"}
    while True:
        page = table.scan(**kwargs)
        ids.extend(item["payment_id"] for item in page.get("Items", []))
        if limit and len(ids) >= limit:
            return ids[:limit]
        key = page.get("LastEvaluatedKey")
        if not key:
            return ids
        kwargs["ExclusiveStartKey"] = key


def audit(payment_id, token, write):
    """Ask the facilitator for this anchor's verdict.

    Returns (outcome, detail). A transport failure is its own outcome rather
    than being folded into `lost`: "we could not check" must never be recorded
    as "the evidence is gone" -- that is the mistake INC-2026-07-21 was, one
    subsystem over.
    """
    req = urllib.request.Request(
        f"{FACILITATOR}/dx402/repair/{payment_id}?write={'true' if write else 'false'}",
        method="POST",
        headers={"Authorization": f"Bearer {token}"},
    )
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as res:
            return json.loads(res.read()).get("outcome", "unknown"), ""
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", "replace")[:200]
        if e.code == 404 and "not found" in body:
            return "unreachable", "no admin token configured on the facilitator"
        return "unreachable", f"HTTP {e.code}: {body}"
    except Exception as e:  # noqa: BLE001 -- any transport failure reads the same
        return "unreachable", str(e)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--repair",
        action="store_true",
        help="correct the records that can be corrected, instead of only reporting",
    )
    ap.add_argument("--limit", type=int, help="audit at most this many anchors")
    args = ap.parse_args()

    token = os.environ.get("DX402_ADMIN_TOKEN", "")
    if not token:
        sys.exit("DX402_ADMIN_TOKEN is not set; the repair route answers 404 without it")

    print(f"facilitator : {FACILITATOR}")
    print(f"table       : {TABLE} ({REGION})")
    print(f"mode        : {'REPAIR' if args.repair else 'report only'}\n")

    ids = scan_payment_ids(args.limit)
    print(f"{len(ids)} anchors to audit\n")

    counts = {
        "healthy": 0,
        "repaired": 0,
        "repairable": 0,
        "lost": 0,
        "unreachable": 0,
        "unknown": 0,
    }
    broken = []
    for n, payment_id in enumerate(ids, 1):
        outcome, detail = audit(payment_id, token, args.repair)
        counts[outcome] = counts.get(outcome, 0) + 1
        if outcome not in ("healthy",):
            broken.append((payment_id, outcome, detail))
        if n % 50 == 0 or n == len(ids):
            print(f"  {n}/{len(ids)} audited")

    print("\n--- verdict ---")
    for name in ("healthy", "repaired", "repairable", "lost", "unreachable", "unknown"):
        if counts.get(name):
            print(f"  {name:12} {counts[name]}")

    if broken:
        print("\n--- anchors that were not healthy ---")
        for payment_id, outcome, detail in broken[:100]:
            print(f"  {payment_id}  {outcome}{'  ' + detail if detail else ''}")
        if len(broken) > 100:
            print(f"  ... and {len(broken) - 100} more")

    # `lost` is the only outcome nothing can fix, so it is the only one that
    # should fail a run someone is watching.
    if counts.get("lost"):
        print(f"\n{counts['lost']} anchors have no bytes in any store.")
        return 1
    if counts.get("unreachable"):
        print(f"\n{counts['unreachable']} could not be checked; re-run before concluding.")
        return 1
    if counts.get("repairable"):
        print(
            f"\n{counts['repairable']} anchors name a store their bytes are not in "
            "and can be corrected. Re-run with --repair."
        )
        return 1
    print("\nEvery anchor resolves.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
