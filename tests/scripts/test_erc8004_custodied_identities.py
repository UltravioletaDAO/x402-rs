#!/usr/bin/env python3
"""Tests for scripts/erc8004_custodied_identities.py. No network: the chain is a fake node.

    python3 -m unittest discover -s tests/scripts -p 'test_erc8004*.py'
"""

import json
import os
import sys
import unittest
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "scripts"))

import erc8004_custodied_identities as audit  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
CORPUS = json.loads((REPO / "tests" / "fixtures" / "erc8004_agent_uri_cases.json").read_text(encoding="utf-8"))

HOLDER = audit.FACILITATOR_MAINNET
STRANGER = "0x7777777777777777777777777777777777777777"
BAD = "http://198-51-100-7.sslip.io/agent.json"
GOOD = "https://execution.market/agents/0x4444444444444444444444444444444444444444"


class Rules(unittest.TestCase):
    """The same corpus the Rust guard is tested against, so the audit and the guard cannot disagree."""

    def test_every_corpus_case_gets_exactly_its_violations(self):
        cases = CORPUS["cases"]
        self.assertGreaterEqual(len(cases), 40)
        for case in cases:
            with self.subTest(uri=case["uri"]):
                self.assertEqual(audit.violations(case["uri"]), case["violations"])

    def test_the_length_limit_matches_the_corpus(self):
        spec = CORPUS["tooLong"]
        self.assertEqual(spec["totalBytes"], audit.RULES["maxBytes"] + 1)
        too_long = spec["prefix"] + "a" * (spec["totalBytes"] - len(spec["prefix"]))
        self.assertEqual(audit.violations(too_long), spec["violations"])
        at_limit = spec["prefix"] + "a" * (audit.RULES["maxBytes"] - len(spec["prefix"]))
        self.assertEqual(audit.violations(at_limit), [])

    def test_the_retired_uri_is_acceptable(self):
        self.assertEqual(audit.violations(audit.RETIRED_URI), [])

    def test_defang_leaves_nothing_clickable(self):
        self.assertEqual(audit.defang(BAD), "hxxp://198-51-100-7[.]sslip[.]io/agent[.]json")


class ReadOnly(unittest.TestCase):
    def test_a_write_method_is_refused_before_anything_is_sent(self):
        rpc = audit.Rpc("http://127.0.0.1:9/")  # the discard port: a request would fail loudly
        for method in ("eth_sendRawTransaction", "eth_sendTransaction", "eth_sign", "personal_sign"):
            with self.subTest(method=method), self.assertRaises(audit.RpcError) as ctx:
                rpc.call(method, [])
            self.assertIn("only reads", str(ctx.exception))

    def test_the_registry_is_read_from_the_rust_source(self):
        source = (REPO / "src" / "erc8004" / "mod.rs").read_text(encoding="utf-8")
        address = audit.registry_address("base")
        self.assertIn(address.removeprefix("0x"), source)
        self.assertTrue(address.lower().startswith("0x8004"))
        self.assertNotEqual(audit.registry_address("base-sepolia"), address)
        with self.assertRaises(SystemExit):
            audit.registry_address("solana")


def _word(n):
    return "0x" + format(n, "064x")


def _address(a):
    return "0x" + a.lower().removeprefix("0x").rjust(64, "0")


def _string(s):
    data = s.encode()
    padded = data + b"\0" * (-len(data) % 32)
    return "0x" + format(32, "064x") + format(len(data), "064x") + padded.hex()


class FakeNode:
    """Answers the reads the audit makes, and records every method it was asked for."""

    def __init__(self, logs, owners, uris, max_range=None):
        self.logs, self.owners, self.uris, self.max_range = logs, owners, uris, max_range
        self.methods = []

    def call(self, method, params):
        self.methods.append(method)
        if method == "eth_getLogs":
            f = params[0]
            lo, hi = int(f["fromBlock"], 16), int(f["toBlock"], 16)
            if self.max_range and hi - lo + 1 > self.max_range:
                raise audit.RpcError("query exceeds max block range")
            assert f["topics"][0] == audit.TRANSFER_TOPIC and f["topics"][2] == _address(HOLDER)
            return [
                {"topics": [audit.TRANSFER_TOPIC, _address("0x0"), _address(HOLDER), _word(tid)],
                 "blockNumber": hex(block)}
                for tid, block in self.logs if lo <= block <= hi
            ]
        if method == "eth_call":
            data = params[0]["data"][2:]
            selector, arg = data[:8], int(data[8:], 16)
            if selector == audit.SEL_OWNER_OF:
                if arg not in self.owners:
                    raise audit.RpcError("execution reverted")
                return _address(self.owners[arg])
            if selector == audit.SEL_TOKEN_URI:
                return _string(self.uris[arg])
        raise AssertionError(f"unexpected {method} {params}")


class Enumeration(unittest.TestCase):
    def setUp(self):
        # 1: ours, bad URI. 2: arrived and left. 3: ours, good URI. 4: ours, already retired.
        # 5: burned (ownerOf reverts). 1 arrives twice: the later block wins.
        self.node = FakeNode(
            logs=[(1, 100), (2, 150), (3, 20_050), (4, 30_000), (5, 30_001), (1, 40_000)],
            owners={1: HOLDER, 2: STRANGER, 3: HOLDER.lower(), 4: HOLDER},
            uris={1: BAD, 3: GOOD, 4: audit.RETIRED_URI},
            max_range=5_000,
        )

    def scan(self):
        received = audit.received_token_ids(self.node, "0xregistry", HOLDER, 0, 50_000, 50_000)
        return received, audit.custodied(self.node, "0xregistry", HOLDER, received)

    def test_only_identities_still_held_are_listed(self):
        received, rows = self.scan()
        self.assertEqual(received[1], 40_000)
        self.assertEqual([r["agentId"] for r in rows], ["1", "3", "4"])

    def test_the_bad_uri_is_flagged_and_the_others_are_not(self):
        _, rows = self.scan()
        by_id = {r["agentId"]: r for r in rows}
        self.assertEqual(by_id["1"]["violations"], ["agent_uri_scheme", "agent_uri_embedded_ip"])
        self.assertEqual(by_id["3"]["violations"], [])
        self.assertTrue(by_id["4"]["retired"])
        self.assertEqual(by_id["4"]["violations"], [])

    def test_a_refused_range_is_halved_not_skipped(self):
        received, _ = self.scan()
        self.assertEqual(set(received), {1, 2, 3, 4, 5})
        self.assertGreater(self.node.methods.count("eth_getLogs"), 10)

    def test_every_method_used_is_a_read(self):
        self.scan()
        self.assertTrue(set(self.node.methods) <= audit.Rpc.READ_METHODS, self.node.methods)

    def test_the_table_defangs_only_flagged_uris(self):
        _, rows = self.scan()
        table = audit.markdown(rows, raw=False)
        self.assertNotIn(BAD, table)
        self.assertIn(audit.defang(BAD), table)
        self.assertIn(GOOD, table)
        self.assertIn(BAD, audit.markdown(rows, raw=True))


class Classify(unittest.TestCase):
    def test_classify_exit_codes(self):
        import contextlib
        import io

        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(audit.main(["--classify", BAD]), 2)
            self.assertEqual(audit.main(["--classify", GOOD]), 0)


if __name__ == "__main__":
    unittest.main()
