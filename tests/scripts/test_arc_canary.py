#!/usr/bin/env python3
"""Tests for the gas guard in scripts/arc_canary.py.

Run standalone (no pytest needed; the canary's own eth-account and eth-utils
must be importable):

    python3 tests/scripts/test_arc_canary.py

The spike is the real one: Arc mainnet's base fee was 81.642861159 gwei at
block 21,205,139 (2026-09-16 19:12:57Z; eth_getBlockByNumber, read 2026-09-23),
when the fixed 50 gwei guard made the release check refuse to run with nothing
wrong. Arc's minimum, and its usual base fee, is 20 gwei.
"""

import os
import sys
import unittest

sys.path.insert(
    0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "scripts")
)

import arc_canary  # noqa: E402

GWEI = 10**9
SPIKE = 81_642_861_159


class GasGuard(unittest.TestCase):
    def test_the_usual_quote_passes(self):
        self.assertIsNone(arc_canary.gas_guard(20 * GWEI, 20 * GWEI))

    def test_the_measured_spike_no_longer_blocks_the_canary(self):
        # The old guard refused anything above 50 gwei.
        self.assertGreater(SPIKE, 50 * GWEI)
        self.assertIsNone(arc_canary.gas_guard(SPIKE, SPIKE))

    def test_a_quote_far_above_the_base_fee_is_refused(self):
        refusal = arc_canary.gas_guard(61 * GWEI, 20 * GWEI)
        self.assertIsNotNone(refusal)
        self.assertIn("3x the 20 gwei base fee", refusal)

    def test_the_limit_is_three_times_the_base_fee(self):
        self.assertIsNone(arc_canary.gas_guard(60 * GWEI, 20 * GWEI))
        self.assertIsNotNone(arc_canary.gas_guard(60 * GWEI + 1, 20 * GWEI))

    def test_the_absolute_ceiling_holds_however_high_the_base_fee(self):
        base = 150 * GWEI  # 3x would allow 450 gwei
        self.assertIsNone(arc_canary.gas_guard(arc_canary.GAS_PRICE_CEILING_WEI, base))
        refusal = arc_canary.gas_guard(arc_canary.GAS_PRICE_CEILING_WEI + 1, base)
        self.assertIsNotNone(refusal)
        self.assertIn("at most 200", refusal)

    def test_no_base_fee_is_a_refusal_not_a_pass(self):
        self.assertIsNotNone(arc_canary.gas_guard(20 * GWEI, None))
        self.assertIsNotNone(arc_canary.gas_guard(20 * GWEI, 0))


if __name__ == "__main__":
    unittest.main()
