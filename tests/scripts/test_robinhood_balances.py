"""Robinhood Chain mainnet reads its balance from the private RPC alone, without AWS or RPC calls.

Owner's rule (2026-11-05): QuickNode's private endpoint, never a public one, with no public
default. The balances Lambda takes it from the `robinhood` key of facilitator-rpc-mainnet; without
it, or with the private one down, Robinhood is not measured (a null balance) instead of being read
from the official keyless endpoint. The URLs below are stand-ins.
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
PUBLIC_MAINNET_HOST = "rpc.mainnet.chain.robinhood.com"
PUBLIC_TESTNET = "https://rpc.testnet.chain.robinhood.com"


class RobinhoodBalancesTest(unittest.TestCase):
    def configs(self, private: dict):
        asked = []

        def get_private_rpc(key):
            asked.append(key)
            return private.get(key)

        out = io.StringIO()
        with patch.dict(os.environ, {}, clear=True), patch.object(
            balances, "get_private_rpc", side_effect=get_private_rpc
        ), redirect_stdout(out):
            configs = balances.get_network_configs()
        self.log = out.getvalue()
        return configs, asked

    @staticmethod
    def rpcs(config: dict) -> list:
        return [r for r in config["rpcs"] if r]

    def read(self, config: dict, rpc) -> tuple:
        """fetch_evm_balance with fetch_json replaced; returns (result, urls asked, log)."""
        urls = []

        def recorded(url, data, **kwargs):
            urls.append(url)
            return rpc(url, data)

        out = io.StringIO()
        with patch.object(balances, "fetch_json", side_effect=recorded), redirect_stdout(out):
            got = balances.fetch_evm_balance("robinhood-mainnet", config)
        return got, urls, out.getvalue()

    def test_mainnet_asks_the_secret_for_its_robinhood_key(self):
        _, asked = self.configs({})
        self.assertIn("robinhood", asked)

    def test_mainnet_reads_the_private_rpc_alone(self):
        configs, _ = self.configs({"robinhood": PRIVATE})
        self.assertEqual(self.rpcs(configs["robinhood-mainnet"]), [PRIVATE])
        self.assertEqual(configs["robinhood-mainnet"]["address"], balances.MAINNET_ADDRESS)

    def test_the_balance_is_read_from_the_private_rpc(self):
        configs, _ = self.configs({"robinhood": PRIVATE})
        got, urls, _ = self.read(configs["robinhood-mainnet"], lambda url, data: {"result": hex(10**16)})
        self.assertEqual(got, ("robinhood-mainnet", "0.0100"))
        self.assertEqual(urls, [PRIVATE])

    def test_without_the_key_robinhood_is_not_measured(self):
        configs, _ = self.configs({})
        self.assertEqual(self.rpcs(configs["robinhood-mainnet"]), [])
        self.assertIn("No private RPC for robinhood, not measured", self.log)
        got, urls, _ = self.read(configs["robinhood-mainnet"], lambda url, data: {"result": hex(10**16)})
        self.assertEqual(got, ("robinhood-mainnet", None))
        self.assertEqual(urls, [])

    def test_the_public_endpoint_is_never_a_robinhood_mainnet_rpc(self):
        for private in ({}, {"robinhood": PRIVATE}):
            with self.subTest(private=private):
                configs, _ = self.configs(private)
                self.assertFalse(
                    any(PUBLIC_MAINNET_HOST in r for r in self.rpcs(configs["robinhood-mainnet"]))
                )
        self.assertNotIn("robinhood", balances.PUBLIC_RPCS)

    def test_a_failed_private_read_is_not_measured_and_logs_the_host_alone(self):
        configs, _ = self.configs({"robinhood": PRIVATE_WITH_KEY})

        def rpc(url, data):
            raise TimeoutError("timed out")

        got, urls, log = self.read(configs["robinhood-mainnet"], rpc)
        self.assertEqual(got, ("robinhood-mainnet", None))
        self.assertEqual(urls, [PRIVATE_WITH_KEY])
        self.assertIn("rh.invalid", log)
        self.assertNotIn("KEYPART", log)

    def test_the_error_text_never_carries_the_url_either(self):
        configs, _ = self.configs({"robinhood": NO_SCHEME_WITH_KEY})

        def rpc(url, data):
            urllib.request.Request(url, data=data)  # the ValueError urllib raises, URL quoted
            return {"result": hex(10**16)}

        got, _, log = self.read(configs["robinhood-mainnet"], rpc)
        self.assertEqual(got, ("robinhood-mainnet", None))
        self.assertIn("unknown url type", log)
        self.assertNotIn("KEYPART", log)

    def test_another_networks_key_is_not_read_as_robinhood(self):
        configs, _ = self.configs({"arbitrum": PRIVATE})
        self.assertEqual(self.rpcs(configs["robinhood-mainnet"]), [])

    def test_the_other_mainnets_keep_their_public_fallback(self):
        # Out of this change's scope (owner's follow-up): only Robinhood loses it.
        configs, _ = self.configs({})
        self.assertEqual(self.rpcs(configs["base-mainnet"])[-1], balances.PUBLIC_RPCS["base"])
        self.assertIn("No private RPC for base, will use public", self.log)

    def test_the_testnet_keeps_its_public_rpc_alone(self):
        configs, asked = self.configs({"robinhood": PRIVATE})
        self.assertEqual(self.rpcs(configs["robinhood-testnet"]), [PUBLIC_TESTNET])
        self.assertEqual(configs["robinhood-testnet"]["address"], balances.TESTNET_ADDRESS)
        self.assertNotIn("robinhood-testnet", asked)


if __name__ == "__main__":
    unittest.main()
