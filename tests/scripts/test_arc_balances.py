"""Arc monitor checks without AWS, RPC calls, keys or funding."""
import importlib.util
import json
import os
from pathlib import Path
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "arc_balances", Path(__file__).resolve().parents[2] / "lambda/balances/handler.py"
)
balances = importlib.util.module_from_spec(spec)
spec.loader.exec_module(balances)


class ArcBalancesTest(unittest.TestCase):
    def configs(self, env):
        with patch.dict(os.environ, env, clear=True), patch.object(
            balances, "get_private_rpc", return_value=None
        ):
            return balances.get_network_configs()

    def test_networks_are_enabled_independently(self):
        for enabled in ({}, {"RPC_URL_ARC": "https://main.example"},
                        {"RPC_URL_ARC_TESTNET": "https://test.example"}):
            with self.subTest(enabled=enabled):
                configs = self.configs(enabled)
                self.assertEqual("arc-mainnet" in configs, "RPC_URL_ARC" in enabled)
                self.assertEqual("arc-testnet" in configs, "RPC_URL_ARC_TESTNET" in enabled)
        configs = self.configs({"RPC_URL_ARC": "https://main.example",
                               "RPC_URL_ARC_TESTNET": "https://test.example"})
        self.assertEqual(configs["arc-mainnet"]["address"], balances.MAINNET_ADDRESS)
        self.assertEqual(configs["arc-testnet"]["address"], balances.TESTNET_ADDRESS)
        self.assertNotEqual(configs["arc-mainnet"]["chain_id"], configs["arc-testnet"]["chain_id"])

    def test_reads_one_native_usdc_balance_at_18_decimals(self):
        methods = []

        def rpc(url, data, **kwargs):
            method = json.loads(data)["method"]
            methods.append(method)
            if method == "eth_chainId":
                return {"result": hex(5042)}
            if method == "eth_getBalance":
                return {"result": hex(13_489_266_029_671_387_940)}
            self.fail("A second ERC-20 balance would double-count the same USDC")

        config = self.configs({"RPC_URL_ARC": "https://main.example"})["arc-mainnet"]
        with patch.object(balances, "fetch_json", side_effect=rpc):
            network, amount = balances.fetch_evm_balance("arc-mainnet", config)
        self.assertEqual(network, "arc-mainnet")
        self.assertGreater(float(amount), 13.489)
        self.assertLess(float(amount), 13.49)
        self.assertEqual(methods, ["eth_chainId", "eth_getBalance"])

    def test_wrong_network_never_becomes_a_healthy_balance(self):
        config = self.configs({"RPC_URL_ARC": "https://test.example"})["arc-mainnet"]
        with patch.object(balances, "fetch_json", return_value={"result": hex(5042002)}) as rpc:
            self.assertEqual(balances.fetch_evm_balance("arc-mainnet", config), ("arc-mainnet", None))
        self.assertEqual(rpc.call_count, 1)


if __name__ == "__main__":
    unittest.main()
