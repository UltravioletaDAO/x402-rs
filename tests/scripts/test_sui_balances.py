"""Sui balance reads of the balances Lambda, without network: gRPC-web over the
facilitator's endpoint list, then GraphQL. No JSON-RPC (no public Sui node serves it)."""
import importlib.util
import json
import os
from pathlib import Path
import unittest
from unittest.mock import patch
import urllib.error

spec = importlib.util.spec_from_file_location(
    "sui_balances", Path(__file__).resolve().parents[2] / "lambda/balances/handler.py"
)
balances = importlib.util.module_from_spec(spec)
spec.loader.exec_module(balances)

OWNER = "0x" + "ab" * 32
MIST = 20814852387


def pb_uint(field, value):
    return balances._pb_varint(field << 3) + balances._pb_varint(value)


def pb_message(field, payload):
    return balances._pb_varint((field << 3) | 2) + balances._pb_varint(len(payload)) + payload


def frame(flag, payload):
    return bytes([flag]) + len(payload).to_bytes(4, "big") + payload


def balance_body(total=MIST, status="0"):
    balance = balances._pb_string(1, "0x2::sui::SUI") + pb_uint(3, total)
    trailers = f"grpc-status:{status}\r\ngrpc-message:\r\n".encode()
    return frame(0x00, pb_message(1, balance)) + frame(0x80, trailers)


class FakeResponse:
    def __init__(self, body, headers=None):
        self._body = body
        self.headers = headers or {}

    def read(self):
        return self._body

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


class SuiEndpointListTest(unittest.TestCase):
    def test_unset_or_blank_gives_the_facilitator_defaults(self):
        for raw in (None, "", "  ", " , ,"):
            env = {} if raw is None else {"RPC_URL_SUI": raw}
            with patch.dict(os.environ, env, clear=True):
                self.assertEqual(
                    balances.sui_grpc_endpoints("RPC_URL_SUI", "sui-mainnet"),
                    ["https://fullnode.mainnet.sui.io:443", "https://sui-grpc.publicnode.com:443"],
                )
        with patch.dict(os.environ, {}, clear=True):
            testnet = balances.sui_grpc_endpoints("RPC_URL_SUI_TESTNET", "sui-testnet")
        self.assertEqual(len(testnet), 2)
        self.assertTrue(all("testnet" in url for url in testnet), testnet)

    def test_a_list_is_trimmed_kept_in_order_and_without_repeats(self):
        raw = " https://a.example:443 , https://b.example/ ,HTTPS://A.EXAMPLE:443/, ftp://x.example, junk "
        with patch.dict(os.environ, {"RPC_URL_SUI": raw}, clear=True):
            self.assertEqual(
                balances.sui_grpc_endpoints("RPC_URL_SUI", "sui-mainnet"),
                ["https://a.example:443", "https://b.example/"],
            )

    def test_garbage_alone_gives_the_defaults(self):
        with patch.dict(os.environ, {"RPC_URL_SUI": "sui-rpc.publicnode.com, wss://x"}, clear=True):
            self.assertEqual(
                balances.sui_grpc_endpoints("RPC_URL_SUI", "sui-mainnet"),
                balances.SUI_GRPC_DEFAULTS["sui-mainnet"].split(","),
            )

    def test_the_configs_carry_no_json_rpc_url(self):
        env = {"RPC_URL_SUI": "https://one.example:443,https://two.example:443"}
        with patch.dict(os.environ, env, clear=True), patch.object(balances, "get_private_rpc", return_value=None):
            configs = balances.get_network_configs()
        for network in ("sui-mainnet", "sui-testnet"):
            self.assertNotIn("rpcs", configs[network])
            self.assertEqual(configs[network]["type"], "sui")
        self.assertEqual(
            configs["sui-mainnet"]["grpc"], ["https://one.example:443", "https://two.example:443"]
        )
        self.assertEqual(configs["sui-mainnet"]["graphql"], ["https://graphql.mainnet.sui.io/graphql"])
        self.assertEqual(configs["sui-testnet"]["graphql"], ["https://graphql.testnet.sui.io/graphql"])


class SuiGrpcWebTest(unittest.TestCase):
    def test_get_balance_request_and_answer(self):
        seen = {}

        def urlopen(req, timeout):
            seen["url"] = req.full_url
            seen["type"] = req.get_header("Content-type")
            seen["body"] = req.data
            return FakeResponse(balance_body())

        with patch.object(balances.urllib.request, "urlopen", urlopen):
            total = balances.sui_grpc_web_balance("https://fullnode.mainnet.sui.io:443/", OWNER)
        self.assertEqual(total, MIST)
        self.assertEqual(
            seen["url"], "https://fullnode.mainnet.sui.io:443/sui.rpc.v2.StateService/GetBalance"
        )
        self.assertEqual(seen["type"], "application/grpc-web+proto")
        body = seen["body"]
        self.assertEqual(body[0], 0)
        self.assertEqual(int.from_bytes(body[1:5], "big"), len(body) - 5)
        fields = balances._pb_fields(body[5:])
        self.assertEqual(fields[1], [OWNER.encode()])
        self.assertEqual(fields[2], [b"0x2::sui::SUI"])

    def test_a_zero_balance_is_zero_not_an_error(self):
        with patch.object(balances.urllib.request, "urlopen", lambda req, timeout: FakeResponse(balance_body(total=0))):
            self.assertEqual(balances.sui_grpc_web_balance("https://x.example", OWNER), 0)

    def test_a_non_ok_status_is_an_error(self):
        cases = [
            FakeResponse(balance_body(status="14")),
            FakeResponse(b"", headers={"grpc-status": "12"}),
            FakeResponse(b""),
            FakeResponse(frame(0x80, b"grpc-status:0\r\n")),
            FakeResponse(frame(0x00, b"") + frame(0x80, b"grpc-status:0\r\n")),
            FakeResponse(frame(0x00, pb_message(1, balances._pb_string(1, "0x2::sui::SUI"))) + frame(0x80, b"grpc-status:0\r\n")),
        ]
        for response in cases:
            with patch.object(balances.urllib.request, "urlopen", lambda req, timeout, r=response: r):
                with self.assertRaises(balances.SuiReadError):
                    balances.sui_grpc_web_balance("https://x.example", OWNER)

    def test_a_truncated_message_is_an_error(self):
        with self.assertRaises(balances.SuiReadError):
            balances._pb_fields(b"\x0a\x05abc")


class SuiGraphqlTest(unittest.TestCase):
    def test_total_balance_and_its_query(self):
        seen = {}

        def fetch_json(url, data, timeout):
            seen["url"], seen["payload"] = url, json.loads(data)
            return {"data": {"address": {"balance": {"totalBalance": str(MIST)}}}}

        with patch.object(balances, "fetch_json", fetch_json):
            self.assertEqual(balances.sui_graphql_balance("https://graphql.mainnet.sui.io/graphql", OWNER), MIST)
        self.assertEqual(seen["payload"]["variables"], {"a": OWNER})
        self.assertIn("0x2::sui::SUI", seen["payload"]["query"])

    def test_errors_and_missing_rows(self):
        with patch.object(balances, "fetch_json", return_value={"errors": [{"message": "x"}]}):
            with self.assertRaises(balances.SuiReadError):
                balances.sui_graphql_balance("https://g.example", OWNER)
        with patch.object(balances, "fetch_json", return_value={"data": {"address": None}}):
            with self.assertRaises(balances.SuiReadError):
                balances.sui_graphql_balance("https://g.example", OWNER)
        with patch.object(balances, "fetch_json", return_value={"data": {"address": {"balance": None}}}):
            self.assertEqual(balances.sui_graphql_balance("https://g.example", OWNER), 0)


class SuiFallbackTest(unittest.TestCase):
    CONFIG = {
        "grpc": ["https://first.example:443", "https://second.example:443"],
        "graphql": ["https://graphql.example/graphql"],
        "address": OWNER,
        "type": "sui",
    }

    def test_first_answer_wins_in_order(self):
        calls = []

        def grpc(url, owner):
            calls.append(("grpc", url))
            if url.startswith("https://first"):
                raise urllib.error.HTTPError(url, 415, "Unsupported Media Type", {}, None)
            return 1_000_000_000

        def graphql(url, owner):
            calls.append(("graphql", url))
            return 5

        with patch.object(balances, "sui_grpc_web_balance", grpc), patch.object(balances, "sui_graphql_balance", graphql):
            self.assertEqual(balances.fetch_sui_balance("sui-testnet", self.CONFIG), ("sui-testnet", "1.0000"))
        self.assertEqual(calls, [("grpc", "https://first.example:443"), ("grpc", "https://second.example:443")])

    def test_graphql_after_every_grpc_endpoint_failed(self):
        def grpc(url, owner):
            raise balances.SuiReadError("grpc-status 14")

        with patch.object(balances, "sui_grpc_web_balance", grpc), patch.object(balances, "sui_graphql_balance", lambda url, owner: MIST):
            self.assertEqual(balances.fetch_sui_balance("sui-mainnet", self.CONFIG), ("sui-mainnet", "20.8149"))

    def test_nothing_answers_is_not_measured(self):
        def fail(url, owner):
            raise TimeoutError()

        with patch.object(balances, "sui_grpc_web_balance", fail), patch.object(balances, "sui_graphql_balance", fail):
            self.assertEqual(balances.fetch_sui_balance("sui-mainnet", self.CONFIG), ("sui-mainnet", None))


if __name__ == "__main__":
    unittest.main()
