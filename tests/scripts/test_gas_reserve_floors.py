#!/usr/bin/env python3
"""Tests for scripts/gas_reserve_floors.py. No network.

Run standalone (no pytest needed):

    python3 tests/scripts/test_gas_reserve_floors.py

alerts.tf divides by each fee cap, so a zero in the map it pastes fails the
Terraform plan after the merge (refutation of PR #99, 2026-09-23). The script
must never print one, nor a map missing a chain, which would remove that
chain's alarms.
"""

import os
import sys
import unittest

sys.path.insert(
    0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "scripts")
)

import gas_reserve_floors as floors  # noqa: E402

GWEI = floors.GWEI


class FeeCap(unittest.TestCase):
    def test_the_send_path_formula(self):
        # Arc on its 20 gwei minimum: 2 x 20 gwei + the 1 mwei tip floor.
        cap = floors.fee_cap(20 * GWEI, 0, floors.FLOORS["arc-mainnet"])
        self.assertEqual(cap, 40 * GWEI + 1_000_000)
        # Ethereum in a trough still reserves its 5 gwei floor.
        self.assertEqual(floors.fee_cap(GWEI // 10, None, floors.FLOORS["ethereum-mainnet"]), 5 * GWEI)


class RoundUp(unittest.TestCase):
    def test_never_rounds_down(self):
        self.assertEqual(floors.round_up(40.001), 40.1)
        self.assertEqual(floors.round_up(0.0110001), 0.0111)

    def test_zero_is_an_error_not_a_zero(self):
        for value in (0, 0.0, -1):
            with self.assertRaises(ValueError):
                floors.round_up(value)


class RenderHcl(unittest.TestCase):
    ROWS = [{"chain": "arc-mainnet", "fee_cap_gwei": 40.1},
            {"chain": "base-mainnet", "fee_cap_gwei": 0.011}]

    def test_prints_the_map(self):
        block = floors.render_hcl(self.ROWS, "2026-09-23T08:52Z", [])
        self.assertIn('"arc-mainnet"  = 40.1', block)
        self.assertIn('"base-mainnet" = 0.011', block)

    def test_refuses_a_zero_fee_cap(self):
        rows = self.ROWS + [{"chain": "zero-mainnet", "fee_cap_gwei": 0.0}]
        with self.assertRaises(ValueError):
            floors.render_hcl(rows, "t", [])

    def test_refuses_a_partial_map(self):
        with self.assertRaises(ValueError):
            floors.render_hcl(self.ROWS, "t", ["polygon-mainnet"])


if __name__ == "__main__":
    unittest.main()
