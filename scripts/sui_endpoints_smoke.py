#!/usr/bin/env python3
"""Read-only smoke of the Sui endpoints the balances Lambda reads, before a deploy.

For each network: every gRPC endpoint of the list (RPC_URL_SUI / RPC_URL_SUI_TESTNET,
or the defaults) over gRPC-web, and the GraphQL endpoint, each asked for the SUI balance
of the facilitator's public wallet with the Lambda's own functions
(lambda/balances/handler.py). Nothing is signed or sent.

The facilitator's side (plain gRPC over HTTP/2: chain id, gas price, balance, objects and
the digest lookup the execution relies on) is the ignored Rust test:

    cargo test --locked -p x402-rs --features sui -- --ignored sui_endpoints_live_smoke --nocapture

Exit 0 when every network has at least one endpoint that answers; 1 otherwise. An
endpoint that does not speak gRPC-web (publicnode answers 415) is reported, not fatal.

    python3 scripts/sui_endpoints_smoke.py
"""
import importlib.util
import sys
import time
from pathlib import Path

HANDLER = Path(__file__).resolve().parents[1] / "lambda" / "balances" / "handler.py"


def load_handler():
    spec = importlib.util.spec_from_file_location("balances_handler", HANDLER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> int:
    handler = load_handler()
    networks = {
        "sui-mainnet": ("RPC_URL_SUI", handler.SUI_MAINNET_ADDRESS),
        "sui-testnet": ("RPC_URL_SUI_TESTNET", handler.SUI_TESTNET_ADDRESS),
    }
    unread = []
    for network, (env_var, wallet) in networks.items():
        attempts = [("gRPC-web", url, handler.sui_grpc_web_balance)
                    for url in handler.sui_grpc_endpoints(env_var, network)]
        attempts += [("GraphQL", url, handler.sui_graphql_balance)
                     for url in handler.SUI_GRAPHQL_URLS[network]]
        answered = 0
        for transport, url, read in attempts:
            started = time.monotonic()
            try:
                mist = read(url, wallet)
                ms = int((time.monotonic() - started) * 1000)
                print(f"OK   {network:12} {transport:8} {handler.rpc_host(url):40} "
                      f"{mist / 1e9:.4f} SUI ({ms} ms)")
                answered += 1
            except Exception as error:  # noqa: BLE001 - every failure is a row
                ms = int((time.monotonic() - started) * 1000)
                print(f"FAIL {network:12} {transport:8} {handler.rpc_host(url):40} "
                      f"{handler.rpc_error(error)} ({ms} ms)")
        if not answered:
            unread.append(network)
    if unread:
        print(f"No endpoint answered for: {', '.join(unread)}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
