"""Facilitator/Chains carries only the chains an alarm reads (COSTO-X402 B7)."""
import importlib.util
import os
import re
import sys
import types
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
TF = ROOT / "terraform/environments/production"
spec = importlib.util.spec_from_file_location("chain_metrics_balances", ROOT / "lambda/balances/handler.py")
balances = importlib.util.module_from_spec(spec)
spec.loader.exec_module(balances)


def hcl_map_keys(text: str, name: str) -> set[str]:
    """Keys of a flat `name = { "k" = v ... }` map in a locals block."""
    match = re.search(rf"^\s*{name}\s*=\s*\{{(.*?)^\s*\}}", text, re.S | re.M)
    if match is None:
        raise AssertionError(f"{name} not found in alerts.tf")
    return set(re.findall(r'^\s*"([a-z0-9-]+)"\s*=', match.group(1), re.M))


def monitored_chains_in_terraform() -> set[str]:
    text = (TF / "alerts.tf").read_text(encoding="utf-8")
    priced = hcl_map_keys(text, "evm_fee_cap_gwei")
    extra = set(re.findall(r'\{\s*"([a-z0-9-]+)"\s*=\s*\{\s*cost\s*=', text))
    hand_set = hcl_map_keys(text, "hand_set_floors")
    return priced | extra | hand_set


class FakeCloudWatch:
    def __init__(self):
        self.calls = []

    def put_metric_data(self, Namespace, MetricData):
        self.calls.append((Namespace, list(MetricData)))


def publish(balances_by_chain: dict) -> list[dict]:
    cw = FakeCloudWatch()
    fake_boto3 = types.SimpleNamespace(client=lambda service: cw)
    with patch.dict(sys.modules, {"boto3": fake_boto3}):
        balances.publish_chain_metrics(balances_by_chain)
    assert all(ns == "Facilitator/Chains" for ns, _ in cw.calls)
    return [datum for _, data in cw.calls for datum in data]


def chains_of(data: list[dict], metric: str) -> set[str]:
    return {d["Dimensions"][0]["Value"] for d in data if d["MetricName"] == metric}


class ChainMetricsTest(unittest.TestCase):
    def test_monitored_set_is_the_alerts_tf_set(self):
        self.assertEqual(set(balances.MONITORED_CHAINS), monitored_chains_in_terraform())
        self.assertEqual(len(balances.MONITORED_CHAINS), 16)

    def test_every_alarm_on_the_namespace_reads_a_published_chain(self):
        for path in TF.glob("*.tf"):
            text = path.read_text(encoding="utf-8")
            if '"Facilitator/Chains"' not in text:
                continue
            for chain in re.findall(r'Chain\s*=\s*"([a-z0-9-]+)"', text):
                self.assertIn(chain, balances.MONITORED_CHAINS, f"{path.name} alarms on {chain}")

    def test_every_monitored_chain_is_one_the_lambda_reads(self):
        env = {"RPC_URL_ARC": "https://arc.invalid", "HEDERA_ACCOUNT_ID_MAINNET": "0.0.1"}
        with patch.dict(os.environ, env, clear=True), patch.object(balances, "get_private_rpc", return_value=None):
            configs = balances.get_network_configs()
        self.assertLessEqual(set(balances.MONITORED_CHAINS), set(configs))
        self.assertGreater(len(set(configs) - balances.MONITORED_CHAINS), 0)

    def test_only_monitored_chains_are_published(self):
        data = publish({
            "base-mainnet": "0.5",
            "sui-mainnet": None,
            "bsc-mainnet": "1.0",
            "base-testnet": "2.0",
            "solana-devnet": None,
            "Base-Mainnet": "3.0",
            " base-mainnet": "3.0",
        })
        self.assertEqual(chains_of(data, "ChainRpcHealthy"), {"base-mainnet", "sui-mainnet"})
        self.assertEqual(chains_of(data, "ChainNativeBalance"), {"base-mainnet"})
        healthy = {d["Dimensions"][0]["Value"]: d["Value"] for d in data if d["MetricName"] == "ChainRpcHealthy"}
        self.assertEqual(healthy, {"base-mainnet": 1.0, "sui-mainnet": 0.0})

    def test_a_full_readout_publishes_at_most_two_series_per_monitored_chain(self):
        env = {"RPC_URL_ARC": "https://arc.invalid", "HEDERA_ACCOUNT_ID_MAINNET": "0.0.1",
               "RPC_URL_ARC_TESTNET": "https://arc-testnet.invalid", "HEDERA_ACCOUNT_ID_TESTNET": "0.0.2"}
        with patch.dict(os.environ, env, clear=True), patch.object(balances, "get_private_rpc", return_value=None):
            configs = balances.get_network_configs()
        data = publish({name: "1.0" for name in configs})
        self.assertEqual(chains_of(data, "ChainRpcHealthy"), set(balances.MONITORED_CHAINS))
        self.assertEqual(len(data), 2 * len(balances.MONITORED_CHAINS))

    def test_the_landing_readout_is_not_filtered(self):
        readout = {"base-mainnet": "0.5", "bsc-mainnet": "1.0", "base-testnet": None}
        publish(readout)
        self.assertEqual(readout, {"base-mainnet": "0.5", "bsc-mainnet": "1.0", "base-testnet": None})


if __name__ == "__main__":
    unittest.main()
