#!/usr/bin/env python3
"""Tests for the evidence walk in scripts/arc_settle_gas_report.py. No network.

Run standalone (no pytest needed):

    python3 tests/scripts/test_arc_settle_gas_report.py

The walk reads every docs/reports/*.json and *.jsonl. The rescued 2026-09-15
evidence of the Arc plan carries `network` as an OBJECT, and the first version of
the walk used it as a dict key: `TypeError`, exit 1, on the very tree that shipped
the report (refutation of PR #99, 2026-09-23).
"""

import os
import sys
import unittest

sys.path.insert(
    0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "scripts")
)

import arc_settle_gas_report as report  # noqa: E402


class RecordedSettles(unittest.TestCase):
    def test_the_walk_survives_every_evidence_file_in_the_repo(self):
        settles = report.recorded_settles()
        # The canary logs and the production acceptance record: 4 per network.
        by_network = {}
        for settle in settles:
            by_network[settle["network"]] = by_network.get(settle["network"], 0) + 1
        self.assertEqual(by_network, {"arc": 4, "arc-testnet": 4})

    def test_a_network_object_is_not_a_settle(self):
        found = []
        node = {"network": {"name": "Arc Testnet", "caip2": "eip155:5042002"},
                "transaction": "0x" + "ab" * 32}
        # The walk is internal to recorded_settles; exercise it through a
        # temporary reports directory.
        import json
        import tempfile
        from pathlib import Path
        with tempfile.TemporaryDirectory() as tmp:
            original = report.REPORTS
            try:
                report.REPORTS = Path(tmp)
                report.REPO = Path(tmp).parent
                (Path(tmp) / "object.json").write_text(json.dumps(node), encoding="utf-8")
                (Path(tmp) / "name.json").write_text(json.dumps(
                    {"network": "eip155:5042", "transaction": "0x" + "cd" * 32}), encoding="utf-8")
                found = report.recorded_settles()
            finally:
                report.REPORTS = original
                report.REPO = original.parent.parent
        self.assertEqual([s["network"] for s in found], ["arc"])


if __name__ == "__main__":
    unittest.main()
