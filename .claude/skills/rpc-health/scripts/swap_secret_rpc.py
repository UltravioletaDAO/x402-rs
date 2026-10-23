#!/usr/bin/env python3
"""Swap one RPC URL inside an AWS Secrets Manager RPC bundle, safely.

`aws secretsmanager update-secret --secret-string` REPLACES the whole JSON
document. facilitator-rpc-mainnet holds ~13 RPC URLs, several with API keys in
them, so a naive write destroys the rest. This does a read-modify-write and
verifies the key set survived.

No secret value is written to disk or printed, except the one key being changed
(RPC URLs with API keys are never the target of this script -- those live under
their own keys and are left untouched). Rollback is the AWSPREVIOUS version
stage:

    aws secretsmanager get-secret-value --secret-id facilitator-rpc-mainnet \
      --region us-east-2 --version-stage AWSPREVIOUS

Usage:
    swap_secret_rpc.py --key celo --url https://celo-rpc.quickapi.com
    swap_secret_rpc.py --key celo --url https://x --secret facilitator-rpc-testnet
"""
import argparse
import json
import subprocess
import sys


def aws(*args):
    return subprocess.run(["aws", *args], capture_output=True, text=True, check=True).stdout


def ssm_reminder(secret, key, region):
    # Decision 266: the task definition reads the SSM copy of each key, not this secret.
    parameter = "/facilitator/" + secret.removeprefix("facilitator-") + "/" + key
    print(f"\nNOT DONE: the facilitator task reads {parameter} from SSM, not this secret.")
    print("Put the same URL there, from a file only you can read holding the URL alone (never the")
    print("URL in argv; CLAUDE.md, 'When adding a new network with premium RPC'):")
    print(f"  aws ssm put-parameter --overwrite --key-id alias/facilitator-bazaar-admin-token --name {parameter} --type SecureString --value file:///dev/shm/rpc-url --region {region}")
    print("It takes effect on the next ECS task start. This secret feeds the balances Lambda.")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--key", required=True, help="lowercase network key, e.g. celo")
    p.add_argument("--url", required=True, help="new RPC URL")
    p.add_argument("--secret", default="facilitator-rpc-mainnet")
    p.add_argument("--region", default="us-east-2")
    p.add_argument("--dry-run", action="store_true")
    args = p.parse_args()

    current = json.loads(aws("secretsmanager", "get-secret-value", "--secret-id", args.secret,
                             "--region", args.region, "--query", "SecretString", "--output", "text"))
    keys_before = sorted(current)
    print(f"{args.secret}: {len(keys_before)} keys -> {keys_before}")

    if args.key not in current:
        print(f"WARNING: key '{args.key}' not present; it will be added")
    else:
        print(f"{args.key} before: {current[args.key]}")

    if current.get(args.key) == args.url:
        print("already set to the requested URL in the secret, nothing to do here")
        ssm_reminder(args.secret, args.key, args.region)
        return 0
    if args.dry_run:
        print(f"[dry-run] would set {args.key} = {args.url}")
        ssm_reminder(args.secret, args.key, args.region)
        return 0

    updated = dict(current)
    updated[args.key] = args.url
    aws("secretsmanager", "update-secret", "--secret-id", args.secret,
        "--region", args.region, "--secret-string", json.dumps(updated))

    after = json.loads(aws("secretsmanager", "get-secret-value", "--secret-id", args.secret,
                           "--region", args.region, "--query", "SecretString", "--output", "text"))
    if sorted(after) != keys_before and set(keys_before) - set(after):
        print("KEYS LOST -- restore from AWSPREVIOUS immediately", file=sys.stderr)
        return 2
    if after[args.key] != args.url:
        print("write did not take effect", file=sys.stderr)
        return 2
    untouched = all(after[k] == current[k] for k in keys_before if k != args.key)
    print(f"{args.key} after : {after[args.key]}")
    print(f"other {len(keys_before) - 1} keys untouched: {untouched}")
    ssm_reminder(args.secret, args.key, args.region)
    return 0 if untouched else 2


if __name__ == "__main__":
    sys.exit(main())
