"""Native balance units and account isolation, without credentials."""
import importlib.util
import os
from pathlib import Path
import unittest
from unittest.mock import patch
spec = importlib.util.spec_from_file_location("hedera_balances", Path(__file__).resolve().parents[2] / "lambda/balances/handler.py")
balances = importlib.util.module_from_spec(spec)
spec.loader.exec_module(balances)

class HederaBalancesTest(unittest.TestCase):
    def test_native_account_balance_is_not_evm_or_usdc_units(self):
        config = {"address": "0.0.1234", "mirror": "https://testnet.mirrornode.hedera.com"}
        data = {"account": "0.0.1234", "deleted": False, "balance": {"balance": 100000001, "tokens": [{"token_id": "0.0.429274", "balance": 999999999}]}}
        with patch.object(balances, "fetch_json", return_value=data):
            self.assertEqual(balances.fetch_hedera_balance("hedera-testnet", config), ("hedera-testnet", "1.00000001"))
        data["account"] = "0.0.1235"
        with patch.object(balances, "fetch_json", return_value=data):
            self.assertEqual(balances.fetch_hedera_balance("hedera-testnet", config), ("hedera-testnet", None))

    def test_only_configured_native_accounts_are_monitored(self):
        with patch.dict(os.environ, {"HEDERA_ACCOUNT_ID_TESTNET": "0.0.1234"}, clear=True), patch.object(balances, "get_private_rpc", return_value=None):
            configs = balances.get_network_configs()
        self.assertEqual(configs["hedera-testnet"]["address"], "0.0.1234")
        self.assertEqual(configs["hedera-testnet"]["type"], "hedera")
        self.assertNotIn("hedera-mainnet", configs)

if __name__ == "__main__":
    unittest.main()
