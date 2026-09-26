#!/usr/bin/env python3
"""Generate an X-UVD-Stack-Key for one stack service, or print a key's digest.

The facilitator never holds a stack key: it is configured with the SHA-256 of
each one (UVD_STACK_KEY_SHA256_<SERVICE>, see src/rate_policy.rs) and the client
holds the key. This script produces both halves without the usual foot-guns --
`echo "$KEY" | shasum` hashes a trailing newline and yields a digest nothing
will ever match -- and without printing the key to the terminal.

  python3 scripts/stack_key.py generate --service karmakadabra --out-dir .
      Writes two files, mode 0600, refusing to overwrite either, and prints only
      the digest and the variable it belongs in:
        karmakadabra-stack-key.facilitator.json  {"sha256": "<64 hex>"}
        karmakadabra-stack-key.client.json       {"key": "uvdsk_..."}
      One per secret: the facilitator's secret holds only the digest, so its
      execution role never reads a key; the client's holds only the key.

  python3 scripts/stack_key.py digest < key.txt
      Reads a key on stdin (surrounding whitespace ignored) and prints its digest.

Never commit the client file: it holds the key (`**/*stack-key*.json` is ignored).
"""

import argparse
import base64
import hashlib
import json
import os
import re
import secrets
import sys

PREFIX = "uvdsk_"
KEY_RE = re.compile(r"^uvdsk_[A-Za-z0-9_-]{43,128}$")
SERVICE_RE = re.compile(r"^[a-z0-9-]{1,40}$")


def new_key() -> str:
    body = base64.urlsafe_b64encode(secrets.token_bytes(32)).rstrip(b"=").decode()
    return PREFIX + body


def digest(key: str) -> str:
    if not KEY_RE.match(key):
        raise SystemExit("not a stack key: expected uvdsk_ and 43-128 base64url characters")
    return hashlib.sha256(key.encode("ascii")).hexdigest()


def env_var(service: str) -> str:
    return "UVD_STACK_KEY_SHA256_" + service.upper().replace("-", "_")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    gen = sub.add_parser("generate")
    gen.add_argument("--service", required=True)
    gen.add_argument("--out-dir", required=True)
    sub.add_parser("digest")
    args = parser.parse_args()

    if args.command == "digest":
        print(digest(sys.stdin.read().strip()))
        return

    if not SERVICE_RE.match(args.service):
        raise SystemExit("service: 1-40 characters of a-z, 0-9 and '-'")
    key = new_key()
    sha256 = digest(key)
    base = os.path.join(args.out_dir, f"{args.service}-stack-key")
    halves = [(f"{base}.facilitator.json", {"sha256": sha256}),
              (f"{base}.client.json", {"key": key})]
    for path, _ in halves:
        if os.path.exists(path):
            raise SystemExit(f"{path} exists; refusing to overwrite a key")
    for path, record in halves:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump(record, f)
            f.write("\n")
    print(f"wrote {halves[0][0]} (the digest: the facilitator's secret)")
    print(f"wrote {halves[1][0]} (mode 0600, holds the key: the client's secret -- never commit it)")
    print(f"{env_var(args.service)}={sha256}")


if __name__ == "__main__":
    main()
