#!/usr/bin/env python3
"""Tests for scripts/polygon_destrabar_cola.py.

Run standalone (no pytest needed):

    python3 tests/scripts/test_polygon_destrabar_cola.py

The numbers in `test_reproduces_the_real_incident` are the real ones read out of
the provider's txpool on 2026-09-10: nonce 1157 priced at 32.247 gwei against a
248 gwei base fee, with 399 correctly priced transactions stacked behind it.
"""

import io
import json
import os
import sys
import unittest
from contextlib import redirect_stdout

sys.path.insert(
    0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "scripts")
)

import polygon_destrabar_cola as pdc  # noqa: E402

GWEI = pdc.GWEI

# The actual stuck head, and one of the transactions queued behind it.
STUCK_HEAD = {"max_fee": 32_247_361_177, "priority": 30_103_229_849, "gas": 206_433}
QUEUED_BEHIND = {"max_fee": 576_969_000_000, "priority": 78_000_000_000, "gas": 357_768}
BASE_FEE_NOW = 248 * GWEI
BASE_FEE_IN_THE_TROUGH = 1_072_065_664  # 1.072 gwei, block 93177231


class TestBump(unittest.TestCase):
    def test_rounds_up_so_the_replacement_clears_bors_threshold(self):
        # bor wants strictly more than +10%. A value that rounds DOWN lands
        # under the threshold and the replacement is refused.
        self.assertEqual(pdc.bump(100, 10.0), 110)
        self.assertEqual(pdc.bump(101, 10.0), 112)  # 111.1 -> 112, not 111
        self.assertEqual(pdc.bump(1, 12.5), 2)  # 1.125 -> 2, not 1

    def test_default_bump_clears_the_ten_percent_rule_for_every_pooled_fee(self):
        for fee in (STUCK_HEAD["max_fee"], QUEUED_BEHIND["max_fee"], 1, 7, 999_999):
            self.assertGreaterEqual(
                pdc.bump(fee, pdc.DEFAULT_BUMP_PCT),
                fee * 1.10,
                f"bumped {fee} does not clear bor's +10% replacement rule",
            )

    def test_rejects_negative(self):
        with self.assertRaises(ValueError):
            pdc.bump(-1, 10.0)


class TestPriceReplacement(unittest.TestCase):
    def setUp(self):
        self.policy = pdc.FeePolicy()

    def test_reproduces_the_real_incident_and_prices_above_the_base_fee(self):
        """The whole point: the replacement must be MINEABLE, unlike the original."""
        max_fee, priority = pdc.price_replacement(
            STUCK_HEAD["max_fee"], STUCK_HEAD["priority"], BASE_FEE_NOW, self.policy
        )
        self.assertGreater(
            max_fee,
            BASE_FEE_NOW,
            "replacement is priced below the base fee -- it would stick exactly "
            "like the transaction it replaces",
        )
        self.assertGreaterEqual(max_fee, STUCK_HEAD["max_fee"] * 1.10)
        self.assertGreaterEqual(priority, STUCK_HEAD["priority"] * 1.10)

    def test_pricing_during_a_base_fee_trough_still_clears_the_steady_state(self):
        """The floor is what makes the fix survive the condition that caused it.

        Priced with only a base-fee multiplier, a transaction built in the
        2026-09-03 trough gets 4 * 1.07 gwei and dies the moment the fee snaps
        back to 250. The floor is the term that does the work here.
        """
        max_fee, _ = pdc.price_replacement(
            STUCK_HEAD["max_fee"],
            STUCK_HEAD["priority"],
            BASE_FEE_IN_THE_TROUGH,
            self.policy,
        )
        self.assertGreaterEqual(
            max_fee,
            250 * GWEI,
            "a replacement priced in the trough would not survive the recovery",
        )

    def test_respects_replace_by_fee_for_an_already_expensive_transaction(self):
        max_fee, priority = pdc.price_replacement(
            QUEUED_BEHIND["max_fee"], QUEUED_BEHIND["priority"], BASE_FEE_NOW, self.policy
        )
        self.assertGreaterEqual(max_fee, QUEUED_BEHIND["max_fee"] * 1.10)
        self.assertGreaterEqual(priority, QUEUED_BEHIND["priority"] * 1.10)

    def test_max_fee_is_never_below_priority(self):
        """A type-2 transaction with maxFee < maxPriorityFee is invalid everywhere."""
        policy = pdc.FeePolicy(
            min_max_fee=0, min_priority=900 * GWEI, base_fee_multiplier=0.0
        )
        max_fee, priority = pdc.price_replacement(1, 1, 0, policy)
        self.assertGreaterEqual(max_fee, priority)

    def test_a_huge_existing_fee_still_wins_over_the_floor(self):
        huge = 5_000 * GWEI
        max_fee, _ = pdc.price_replacement(huge, 1 * GWEI, BASE_FEE_NOW, self.policy)
        self.assertGreaterEqual(max_fee, huge * 1.10)


class TestEffectiveGasPrice(unittest.TestCase):
    def test_charges_base_plus_priority_not_the_cap(self):
        self.assertEqual(
            pdc.effective_gas_price(1000 * GWEI, 40 * GWEI, 248 * GWEI), 288 * GWEI
        )

    def test_cap_binds_when_it_is_lower(self):
        self.assertEqual(
            pdc.effective_gas_price(100 * GWEI, 40 * GWEI, 248 * GWEI), 100 * GWEI
        )


def build_pool(first=1157, count=400):
    """The shape of the real pool: one underpriced head, the rest priced fine."""
    pool = {first: dict(STUCK_HEAD)}
    for i in range(1, count):
        pool[first + i] = dict(QUEUED_BEHIND)
    return pool


class TestPlanReplacements(unittest.TestCase):
    def setUp(self):
        self.pool = build_pool()
        self.policy = pdc.FeePolicy()

    def test_head_mode_replaces_exactly_the_stuck_head(self):
        planned = pdc.plan_replacements(self.pool, 1157, "head", BASE_FEE_NOW, self.policy)
        self.assertEqual([t.nonce for t in planned], [1157])

    def test_cancel_all_covers_every_pooled_nonce_from_the_head_up(self):
        planned = pdc.plan_replacements(
            self.pool, 1157, "cancel-all", BASE_FEE_NOW, self.policy
        )
        self.assertEqual(len(planned), 400)
        self.assertEqual(planned[0].nonce, 1157)
        self.assertEqual(planned[-1].nonce, 1556)

    def test_order_is_strictly_ascending(self):
        """Descending order would ask the node to insert the most expensive
        replacement while the pool is still fully reserved."""
        planned = pdc.plan_replacements(
            self.pool, 1157, "cancel-all", BASE_FEE_NOW, self.policy
        )
        nonces = [t.nonce for t in planned]
        self.assertEqual(nonces, sorted(nonces))
        self.assertEqual(len(set(nonces)), len(nonces))

    def test_ignores_nonces_below_the_accounts_next_nonce(self):
        """Anything below `latest` is already mined; replacing it is impossible."""
        pool = build_pool(first=1157, count=5)
        pool[1150] = dict(QUEUED_BEHIND)
        planned = pdc.plan_replacements(pool, 1157, "cancel-all", BASE_FEE_NOW, self.policy)
        self.assertNotIn(1150, [t.nonce for t in planned])

    def test_refuses_when_the_next_nonce_is_not_pooled(self):
        """Either nothing is stuck, or this node's pool is not the stuck one.

        Both cases need a human, not a transaction: replacing a nonce that no
        pool holds is a plain send, and if there IS a gap it needs a filler.
        """
        with self.assertRaises(ValueError):
            pdc.plan_replacements(self.pool, 1156, "head", BASE_FEE_NOW, self.policy)

    def test_rejects_an_unknown_mode(self):
        with self.assertRaises(ValueError):
            pdc.plan_replacements(self.pool, 1157, "burn-it-down", BASE_FEE_NOW, self.policy)

    def test_every_replacement_is_a_21000_gas_self_transfer(self):
        planned = pdc.plan_replacements(
            self.pool, 1157, "cancel-all", BASE_FEE_NOW, self.policy
        )
        self.assertTrue(all(t.gas == 21_000 for t in planned))


class TestReservationHeadroom(unittest.TestCase):
    """The node re-checks the account's whole pooled cost on every insert."""

    def setUp(self):
        self.pool = build_pool()
        self.reserved = sum(v["gas"] * v["max_fee"] for v in self.pool.values())
        self.policy = pdc.FeePolicy()

    def test_flags_the_momentary_increase_when_replacing_the_cheap_head(self):
        """Replacing the head RAISES the reservation: it is the one cheap entry.

        With almost no free balance this is the step that can fail, and it fails
        first -- so it has to be visible in the dry run, not discovered mid-apply.
        """
        planned = pdc.plan_replacements(self.pool, 1157, "head", BASE_FEE_NOW, self.policy)
        head = planned[0]
        self.assertGreater(head.reserved, head.old_reserved)
        balance = self.reserved  # exactly no free balance
        worst, worst_at = pdc.reservation_headroom(planned, self.reserved, balance)
        self.assertLess(worst, 0)
        self.assertEqual(worst_at.nonce, 1157)

    def test_cancel_all_frees_reservation_as_it_walks_upward(self):
        planned = pdc.plan_replacements(
            self.pool, 1157, "cancel-all", BASE_FEE_NOW, self.policy
        )
        balance = self.reserved + 10**17  # the real 0.062 POL of slack
        worst, _ = pdc.reservation_headroom(planned, self.reserved, balance)
        self.assertGreaterEqual(worst, 0)
        final = self.reserved + sum(t.reserved - t.old_reserved for t in planned)
        self.assertLess(final, self.reserved)


class MockRpc:
    """Stands in for the provider. Records what would have been sent."""

    def __init__(self, pool, base_fee=BASE_FEE_NOW, latest=1157, chain_id=137,
                 balance=82_861_633_384_675_957_709):
        self.pool = pool
        self.base_fee = base_fee
        self.latest = latest
        self.chain_id = chain_id
        self.balance = balance
        self.sent = []
        self.host = "mock.invalid"

    def call(self, method, params=None):
        params = params or []
        if method == "eth_chainId":
            return hex(self.chain_id)
        if method == "eth_getTransactionCount":
            return hex(self.latest if params[1] == "latest"
                       else max(self.pool) + 1 if self.pool else self.latest)
        if method == "eth_getBalance":
            return hex(self.balance)
        if method == "eth_getBlockByNumber":
            return {"baseFeePerGas": hex(self.base_fee), "number": "0x1"}
        if method == "txpool_contentFrom":
            return {
                "pending": {
                    str(n): {
                        "maxFeePerGas": hex(v["max_fee"]),
                        "maxPriorityFeePerGas": hex(v["priority"]),
                        "gas": hex(v["gas"]),
                        "to": "0x" + "11" * 20,
                        "hash": "0x" + "22" * 32,
                    }
                    for n, v in self.pool.items()
                },
                "queued": {},
            }
        if method == "eth_sendRawTransaction":
            self.sent.append(params[0])
            return "0x" + "33" * 32
        raise AssertionError(f"MockRpc got an unexpected call: {method}")


class TestDryRunAgainstMockRpc(unittest.TestCase):
    def setUp(self):
        self.mock = MockRpc(build_pool())
        self._rpc, self._secret, self._price = pdc.Rpc, pdc.read_secret_field, pdc.fetch_pol_price
        pdc.Rpc = lambda url, timeout=45: self.mock
        pdc.read_secret_field = lambda secret_id, field, region: "https://mock.invalid/key"
        pdc.fetch_pol_price = lambda: 0.092382

    def tearDown(self):
        pdc.Rpc, pdc.read_secret_field, pdc.fetch_pol_price = self._rpc, self._secret, self._price

    def run_main(self, argv):
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = pdc.main(argv)
        return code, buf.getvalue()

    def test_dry_run_sends_nothing(self):
        code, out = self.run_main(["--dry-run"])
        self.assertEqual(code, 0)
        self.assertEqual(self.mock.sent, [], "--dry-run must not broadcast anything")
        self.assertIn("nothing was sent", out)

    def test_dry_run_names_the_underpriced_nonce(self):
        _, out = self.run_main(["--dry-run"])
        self.assertIn("1157", out)

    def test_dry_run_reports_a_cost_and_a_plan_size(self):
        _, out = self.run_main(["--dry-run", "--mode", "cancel-all"])
        self.assertIn("400 replacement transaction(s)", out)
        self.assertIn("total charged to the signer", out)

    def test_refuses_a_chain_that_is_not_polygon(self):
        self.mock.chain_id = 8453
        code, _ = self.run_main(["--dry-run"])
        self.assertEqual(code, 2)
        self.assertEqual(self.mock.sent, [])

    def test_reads_the_pool_and_converts_hex_correctly(self):
        pool = pdc.read_pool(self.mock, "0xabc")
        self.assertEqual(pool[1157]["max_fee"], STUCK_HEAD["max_fee"])
        self.assertEqual(pool[1157]["gas"], STUCK_HEAD["gas"])
        self.assertEqual(len(pool), 400)


if __name__ == "__main__":
    unittest.main(verbosity=2)
