"""Offline tests for scripts/bazaar_probe_churn.py's declared-method report.

No network: every input is a local file or an in-memory snapshot.
Run: python -m unittest tests/scripts/test_bazaar_probe_churn.py
"""

import importlib.util
import json
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "bazaar_probe_churn", ROOT / "scripts" / "bazaar_probe_churn.py"
)
churn = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(churn)


def info(method=None, body=None, body_type=None):
    inp = {"type": "http"}
    if method is not None:
        inp["method"] = method
    if body is not None:
        inp["body"] = body
    if body_type is not None:
        inp["bodyType"] = body_type
    return {"bazaar": {"info": {"input": inp}}}


def schema(inp):
    return {"bazaar": {"schema": {"properties": {"input": inp}}}}


class DeclaredProbe(unittest.TestCase):
    def test_same_table_as_the_prober(self):
        # Mirrors `declared_request_reads_every_declaration_in_order` in
        # src/discovery_health.rs.
        for raw, want in [
            (" post ", "POST"), ("put", "PUT"), ("PATCH", "PATCH"), ("get", "GET"),
            ("HEAD", "GET"), ("DELETE", "GET"), ("FETCH", None), ("", None), (42, None),
        ]:
            self.assertEqual(churn.declared_probe(info(raw))[0], want, raw)
        self.assertEqual(churn.declared_probe(None), (None, False))
        self.assertEqual(churn.declared_probe({"other": {}}), (None, False))
        self.assertEqual(churn.declared_probe(info("POST", {"q": 1})), ("POST", True))
        # The schema half, then a body declared without a method.
        self.assertEqual(churn.declared_probe(schema({"type": "http", "method": "GET"}))[0], "GET")
        self.assertEqual(
            churn.declared_probe(schema({"properties": {"method": {"const": "PUT"}}}))[0], "PUT")
        self.assertEqual(
            churn.declared_probe(schema({"properties": {"method": {"enum": ["POST", "GET"]}}}))[0],
            "POST")
        self.assertEqual(
            churn.declared_probe(schema({"properties": {"body": {"type": "object"}}}))[0], "POST")
        self.assertEqual(churn.declared_probe(info(body_type="json"))[0], "POST")
        # info.input.method wins.
        both = {"bazaar": {"info": {"input": {"method": "GET"}},
                           "schema": {"properties": {"input": {"properties": {"body": {}}}}}}}
        self.assertEqual(churn.declared_probe(both)[0], "GET")

    def test_the_captured_feed_page(self):
        page = json.loads(
            (ROOT / "tests" / "fixtures" / "bazaar" / "cdp-pricing-page.json").read_text(
                encoding="utf-8"
            )
        )
        methods = sorted(
            str(churn.declared_probe(i.get("extensions"))[0]) for i in page["items"]
        )
        self.assertEqual(methods, ["GET", "None", "POST", "POST"])

    def test_our_origin_and_opted_out_prefixes_get_a_get(self):
        ext = info("POST", {"a": 1})
        self.assertEqual(
            churn.planned_probe("https://facilitator.ultravioletadao.xyz/register", ext),
            ("GET", False))
        get_only = [("api.seller.example", "/write/")]
        self.assertEqual(
            churn.planned_probe("https://api.seller.example/write/x", ext, get_only),
            ("GET", False))
        self.assertEqual(
            churn.planned_probe("https://api.seller.example/read/x", ext, get_only)[0], "POST")
        self.assertEqual(
            churn.planned_probe("https://u@api.seller.example/write/x", ext, get_only)[0], "POST",
            "a URL with userinfo matches no prefix, as in match_manifest_prefix")
        self.assertEqual(churn.load_get_only(), [], "the shipped manifest opts nobody out")


def rec(status, http, method="absent", typ="http", **extra):
    r = {"status": status, "httpStatus": http, "lastChecked": 1, "type": typ}
    if method != "absent":
        r["method"] = method
    r.update(extra)
    return r


class MethodsReport(unittest.TestCase):
    def setUp(self):
        self.records = {
            "https://a.example/post-quarantined": rec("quarantined", 404, "POST"),
            "https://a.example/post-authgated": rec("auth_gated", 405, "POST"),
            "https://a.example/get-quarantined": rec("quarantined", 404, "GET"),
            "https://b.example/undeclared-405": rec("auth_gated", 405, None),
            "https://b.example/undeclared-400": rec("degraded", 400, None),
            "https://b.example/undeclared-404": rec("quarantined", 404, None),
            "https://c.example/post-drift": rec("quarantined", 402, "POST"),
            "https://c.example/mcp": rec("auth_gated", 405, "POST", typ="mcp"),
            "https://d.example/old-snapshot": rec("alive", 402),
        }

    def test_counts_what_the_change_touches(self):
        m = churn.methods_report(self.records)
        self.assertEqual(m["records"], 9)
        self.assertEqual(
            m["declared"], {"POST": 4, "GET": 1, "none": 3, "unknown": 1}
        )
        self.assertEqual(
            m["get_to_body_method_by_status"],
            {"quarantined": 2, "auth_gated": 1},
            "the mcp endpoint keeps its handshake",
        )
        self.assertEqual(
            m["fallback_candidates_by_status"],
            {"auth_gated": 1, "degraded": 1, "quarantined": 1},
            "a 404 is a fallback candidate too",
        )
        # Two POST listings probed by GET, three fallback candidates, and the
        # drift hold looked at once. Not the GET listing, not the mcp endpoint.
        self.assertEqual(m["reprobed_in_first_cycle"], 6)
        self.assertEqual(m["drift_hold"], 1)
        self.assertEqual(m["drift_hold_hosts"], {"c.example": 1})

    def test_a_kept_reason_beats_the_signature(self):
        records = {
            "https://e.example/recovering": rec(
                "quarantined", 402, None, quarantineReason="fail_streak", probeMethod="GET"),
            "https://e.example/held": rec(
                "quarantined", 404, None, quarantineReason="pay_to_drift", probeMethod="GET"),
        }
        m = churn.methods_report(records)
        self.assertEqual(m["drift_hold_hosts"], {"e.example": 1})
        self.assertEqual(m["reprobed_in_first_cycle"], 0, "both already probed by this build")

    def test_json_output_survives_a_listing_with_no_record(self):
        records = {"https://f.example/x": rec(None, None, "POST")}
        m = churn.methods_report(records)
        json.dumps(m, sort_keys=True)
        self.assertEqual(m["get_to_body_method_by_status"], {"None": 1})

    def test_reads_the_catalog_object_and_the_health_overlay(self):
        catalog = [
            {"url": "https://a.example/x", "type": "http", "extensions": info("POST", {"q": 1})},
            {"url": "https://a.example/y", "type": "http"},
        ]
        health = {
            "https://a.example/x": {"status": "quarantined", "http_status": 404, "last_checked": 5},
            "https://a.example/y": {"status": "auth_gated", "http_status": 405, "last_checked": 5},
        }
        with tempfile.TemporaryDirectory() as d:
            cat = pathlib.Path(d) / "resources.json"
            hp = pathlib.Path(d) / "health.json"
            cat.write_text(json.dumps(catalog), encoding="utf-8")
            hp.write_text(json.dumps(health), encoding="utf-8")
            records = churn.load_records(str(cat), str(hp))
        self.assertEqual(records["https://a.example/x"]["method"], "POST")
        self.assertTrue(records["https://a.example/x"]["hasExample"])
        self.assertEqual(records["https://a.example/y"]["httpStatus"], 405)
        m = churn.methods_report(records)
        self.assertEqual(m["reprobed_in_first_cycle"], 2)


class Compare(unittest.TestCase):
    def test_splits_the_moves_into_the_groups_a_consumer_needs(self):
        before = {"capturedAt": 100, "records": {
            "https://a.example/x": rec("quarantined", 404, "POST"),
            "https://a.example/y": rec("auth_gated", 405, None),
            "https://b.example/z": rec("quarantined", 404, None),
            "https://c.example/w": rec("alive", 402, "GET"),
            "https://d.example/v": rec("quarantined", 404, "POST"),
        }}
        after_records = {
            "https://a.example/x": rec("alive", 402, "POST"),
            "https://a.example/y": rec("alive", 402, None),
            "https://b.example/z": rec("alive", 402, None),
            "https://c.example/w": rec("alive", 402, "GET"),
            "https://d.example/v": rec("degraded", 400, "POST"),
        }
        for r in after_records.values():
            r["lastChecked"] = 50
        c = churn.compare(before, {"capturedAt": 200, "records": after_records})
        self.assertEqual(dict(c["by_method"]["POST"]),
                         {("quarantined", "alive"): 1, ("quarantined", "degraded"): 1})
        self.assertEqual(
            dict(c["groups"]),
            {"quarantined -> visible": 3, "quarantined -> alive/auth_gated": 2,
             "auth_gated -> alive": 1, "unchanged": 1},
        )
        self.assertEqual(c["newly_visible_hosts"], 2, "b and d; a.example was visible already")
        self.assertEqual(c["newly_admissible_hosts"], 1, "only b: d came back degraded")


class Exposure(unittest.TestCase):
    # Mirrors `verified_alive_is_the_declared_request_inside_the_window` and
    # `a_legacy_alive_record_keeps_its_listing_only_on_the_same_probes_reading`
    # in src/discovery_health.rs.
    NOW, WINDOW = 10_000_000, 600

    def ok(self, r):
        return churn.verified_alive(r, self.NOW, self.WINDOW)

    def test_the_owners_rule(self):
        now = self.NOW
        get = dict(probeMethod="GET")
        self.assertTrue(self.ok(rec("alive", 402, "GET", verifiedAt=now - 600, **get)))
        self.assertFalse(self.ok(rec("alive", 402, "GET", verifiedAt=now - 601, **get)),
                         "past the window")
        self.assertFalse(self.ok(rec("alive", 402, "GET", **get)), "never verified")
        for status in ("auth_gated", "degraded", "quarantined", "unprobeable", "unknown", None):
            self.assertFalse(self.ok(rec(status, 402, "GET", verifiedAt=now, **get)), status)
        self.assertFalse(self.ok(rec("alive", 402, "POST", verifiedAt=now, **get)),
                         "a GET answer to a POST listing")
        self.assertTrue(self.ok(rec("alive", 402, "POST", verifiedAt=now, probeMethod="POST")))
        self.assertTrue(self.ok(rec("alive", 402, None, verifiedAt=now, probeMethod="POST")),
                        "a listing that declares nothing counts what the fallback found")

    def test_an_mcp_endpoint_is_verified_by_its_handshake(self):
        # Mirrors `a_challenge_does_not_verify_an_mcp_endpoint_nor_a_handshake_an_http_listing`.
        now = self.NOW
        mcp = dict(typ="mcp")
        self.assertTrue(self.ok(rec("alive", 200, None, verifiedAt=now,
                                    verifiedBy="mcp_handshake", **mcp)))
        self.assertFalse(self.ok(rec("alive", 200, None, verifiedAt=now - 601,
                                     verifiedBy="mcp_handshake", **mcp)), "the same window")
        self.assertFalse(self.ok(rec("alive", 402, None, verifiedAt=now,
                                     verifiedBy="x402_challenge", **mcp)),
                         "a challenge does not verify an MCP endpoint")
        self.assertFalse(self.ok(rec("alive", 200, None, **mcp)), "a handshake with no tool")
        self.assertFalse(self.ok(rec("alive", 402, "GET", verifiedAt=now, probeMethod="GET",
                                     verifiedBy="mcp_handshake")),
                         "nor a handshake an HTTP listing")

    def test_a_legacy_record_needs_the_same_probes_reading(self):
        legacy = dict(lastChecked=self.NOW - 10)
        self.assertTrue(self.ok(rec("alive", 402, "GET", observedAt=self.NOW - 7, **legacy)))
        self.assertFalse(self.ok(rec("alive", 402, "GET", observedAt=self.NOW - 71, **legacy)),
                         "another probe's reading")
        self.assertFalse(self.ok(rec("alive", 402, "GET", **legacy)), "no readable challenge")
        self.assertFalse(self.ok(rec("alive", 402, "POST", observedAt=self.NOW - 7, **legacy)))
        self.assertFalse(
            self.ok(rec("alive", 402, "GET", observedAt=self.NOW - 7, probeMethod="GET", **legacy)),
            "a record this build wrote carries verifiedAt or nothing")
        self.assertFalse(self.ok(rec("alive", 200, "GET", observedAt=self.NOW - 7, **legacy)))

    def test_counts_served_before_and_after(self):
        now = self.NOW
        records = {
            "https://a.example/verified": rec("alive", 402, "GET", verifiedAt=now, probeMethod="GET"),
            "https://a.example/garbage": rec("alive", 402, "GET", probeMethod="GET"),
            "https://a.example/mcp": rec("alive", 200, None, typ="mcp"),
            "https://a.example/wrong": rec("alive", 402, "POST", verifiedAt=now, probeMethod="GET"),
            "https://b.example/auth": rec("auth_gated", 405, "GET"),
            "https://b.example/q": rec("quarantined", 404, "GET"),
            "https://b.example/new": rec(None, None, "GET"),
        }
        e = churn.exposure_report(records, now, self.WINDOW)
        self.assertEqual(e["served_before"], 6, "everything but the quarantined one")
        self.assertEqual(e["served_after"], 1)
        self.assertEqual(e["pending_by_reason"], {
            "alive_no_readable_challenge": 1, "alive_mcp_handshake": 1,
            "alive_to_another_method": 1, "auth_gated": 1, "quarantined": 1, "never_probed": 1,
        })

    def test_reads_the_three_overlays(self):
        catalog = [{"url": "https://a.example/x", "type": "http"}]
        health = {"https://a.example/x": {"status": "alive", "http_status": 402, "last_checked": 100}}
        terms = {"version": 2, "records": {"https://a.example/x": {"observedAt": 101}}}
        with tempfile.TemporaryDirectory() as d:
            paths = {}
            for name, doc in (("resources", catalog), ("health", health), ("terms", terms)):
                paths[name] = pathlib.Path(d) / f"{name}.json"
                paths[name].write_text(json.dumps(doc), encoding="utf-8")
            records = churn.load_records(*(str(paths[n]) for n in ("resources", "health", "terms")))
        self.assertEqual(records["https://a.example/x"]["observedAt"], 101)
        self.assertEqual(churn.exposure_report(records, 200, 600)["served_after"], 1)


if __name__ == "__main__":
    unittest.main()
