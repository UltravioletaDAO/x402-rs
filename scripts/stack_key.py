#!/usr/bin/env python3
"""Generate an X-UVD-Stack-Key for one stack service, or print a key's digest.

The facilitator never holds a stack key: it is configured with the SHA-256 of
each one (UVD_STACK_KEY_SHA256_<SERVICE>, see src/rate_policy.rs) and the client
holds the key. This script produces both halves without the usual foot-guns --
`echo "$KEY" | shasum` hashes a trailing newline and yields a digest nothing
will ever match -- and without printing the key to the terminal.

  python3 scripts/stack_key.py generate --service karmakadabra --out ./kk-stack-key.json
      Writes {"service", "key", "sha256"} to --out with mode 0600 and prints only
      the digest and the variable it belongs in. Refuses to overwrite a file.
      The JSON has the shape of the Secrets Manager secret the handoff describes:
      the client reads `key`, the facilitator reads `sha256`.

  python3 scripts/stack_key.py digest < key.txt
      Reads a key on stdin (surrounding whitespace ignored) and prints its digest.

Never commit the --out file: it holds the key.
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
    gen.add_argument("--out", required=True)
    sub.add_parser("digest")
    args = parser.parse_args()

    if args.command == "digest":
        print(digest(sys.stdin.read().strip()))
        return

    if not SERVICE_RE.match(args.service):
        raise SystemExit("service: 1-40 characters of a-z, 0-9 and '-'")
    key = new_key()
    record = {"service": args.service, "key": key, "sha256": digest(key)}
    try:
        fd = os.open(args.out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        raise SystemExit(f"{args.out} exists; refusing to overwrite a key")
    with os.fdopen(fd, "w") as f:
        json.dump(record, f)
        f.write("\n")
    print(f"wrote {args.out} (mode 0600, holds the key -- never commit it)")
    print(f"{env_var(args.service)}={record['sha256']}")


if __name__ == "__main__":
    main()
