"""Robinhood Chain mainnet reads its balance from the private RPC first, without AWS or RPC calls.

Owner's rule (2026-11-05): QuickNode's private endpoint, never the public one. The balances Lambda
takes it from the `robinhood` key of facilitator-rpc-mainnet, like the other premium mainnets; the
official keyless endpoint is only what it falls back to. The URL below is a stand-in.
"""
from contextlib import redirect_stdout
import importlib.util
import io
import os
from pathlib import Path
import unittest
from unittest.mock import patch
import urllib.request


spec = importlib.util.spec_from_file_location(
    "robinhood_balances", Path(__file__).resolve().parents[2] / "lambda/balances/handler.py"
)
balances = importlib.util.module_from_spec(spec)
spec.loader.exec_module(balances)

PRIVATE = "https://private.invalid/robinhood"
# A short host puts the "key" of the path inside the first 50 characters.
PRIVATE_WITH_KEY = "https://rh.invalid/KEYPART-not-a-real-key/"
# A value with no scheme: urllib's own ValueError quotes it whole.
NO_SCHEME_WITH_KEY = "rh.invalid/KEYPART-not-a-real-key/"
PUBLIC_MAINNET = "https://rpc.mainnet.chain.robinhood.com"
PUBLIC_TESTNET = "https://rpc.testnet.chain.robinhood.com"


class RobinhoodBalancesTest(unittest.TestCase):
    def configs(self, private: dict):
        asked = []

        def get_private_rpc(key):
            asked.append(key)
            return private.get(key)

        with patch.dict(os.environ, {}, clear=True), patch.object(
            balances, "get_private_rpc", side_effect=get_private_rpc
        ):
            return balances.get_network_configs(), asked

    @staticmethod
    def rpcs(config: dict) -> list:
        return [r for r in config["rpcs"] if r]

    def test_mainnet_asks_the_secret_for_its_robinhood_key(self):
        _, asked = self.configs({})
        self.assertIn("robinhood", asked)

    def test_the_private_rpc_comes_before_the_public_one(self):
        configs, _ = self.configs({"robinhood": PRIVATE})
        self.assertEqual(self.rpcs(configs["robinhood-mainnet"]), [PRIVATE, PUBLIC_MAINNET])
        self.assertEqual(configs["robinhood-mainnet"]["address"], balances.MAINNET_ADDRESS)

    def test_the_balance_is_read_from_the_private_rpc(self):
        configs, _ = self.configs({"robinhood": PRIVATE})
        urls = []

        def rpc(url, data, **kwargs):
            urls.append(url)
            return {"result": hex(10**16)}

        with patch.object(balances, "fetch_json", side_effect=rpc):
            got = balances.fetch_evm_balance("robinhood-mainnet", configs["robinhood-mainnet"])
        self.assertEqual(got, ("robinhood-mainnet", "0.0100"))
        self.assertEqual(urls, [PRIVATE])

    def test_a_failed_private_read_falls_back_and_logs_the_host_alone(self):
        configs, _ = self.configs({"robinhood": PRIVATE_WITH_KEY})

        def rpc(url, data, **kwargs):
            if url == PRIVATE_WITH_KEY:
                raise TimeoutError("timed out")
            return {"result": hex(10**16)}

        out = io.StringIO()
        with patch.object(balances, "fetch_json", side_effect=rpc), redirect_stdout(out):
            got = balances.fetch_evm_balance("robinhood-mainnet", configs["robinhood-mainnet"])
        self.assertEqual(got, ("robinhood-mainnet", "0.0100"))
        self.assertIn("rh.invalid", out.getvalue())
        self.assertNotIn("KEYPART", out.getvalue())

    def test_the_error_text_never_carries_the_url_either(self):
        configs, _ = self.configs({"robinhood": NO_SCHEME_WITH_KEY})

        def rpc(url, data, **kwargs):
            if url == NO_SCHEME_WITH_KEY:
                urllib.request.Request(url, data=data)  # the ValueError urllib raises, URL quoted
            return {"result": hex(10**16)}

        out = io.StringIO()
        with patch.object(balances, "fetch_json", side_effect=rpc), redirect_stdout(out):
            got = balances.fetch_evm_balance("robinhood-mainnet", configs["robinhood-mainnet"])
        self.assertEqual(got, ("robinhood-mainnet", "0.0100"))
        self.assertIn("unknown url type", out.getvalue())
        self.assertNotIn("KEYPART", out.getvalue())

    def test_without_the_key_it_falls_back_to_the_public_rpc(self):
        configs, _ = self.configs({})
        self.assertEqual(self.rpcs(configs["robinhood-mainnet"]), [PUBLIC_MAINNET])

    def test_another_networks_key_is_not_read_as_robinhood(self):
        configs, _ = self.configs({"arbitrum": PRIVATE})
        self.assertEqual(self.rpcs(configs["robinhood-mainnet"]), [PUBLIC_MAINNET])

    def test_the_testnet_keeps_its_public_rpc_alone(self):
        configs, asked = self.configs({"robinhood": PRIVATE})
        self.assertEqual(self.rpcs(configs["robinhood-testnet"]), [PUBLIC_TESTNET])
        self.assertEqual(configs["robinhood-testnet"]["address"], balances.TESTNET_ADDRESS)
        self.assertNotIn("robinhood-testnet", asked)


if __name__ == "__main__":
    unittest.main()
