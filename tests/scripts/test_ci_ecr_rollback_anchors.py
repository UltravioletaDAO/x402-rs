"""COSTO-X402 B15: the ECR lifecycle policy and the anchor gate in front of it."""
import importlib.util
import json
import re
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
TF = ROOT / "terraform/environments/production"
spec = importlib.util.spec_from_file_location("ecr_rollback_anchors", ROOT / "scripts/ecr_rollback_anchors.py")
anchors_mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(anchors_mod)

REG = "ACCOUNT.dkr.ecr.us-east-2.amazonaws.com"
INDEX = "application/vnd.oci.image.index.v1+json"


def digest(n: int) -> str:
    return "sha256:" + f"{n:064x}"


class AwsError(Exception):
    def __init__(self, code):
        super().__init__(code)
        self.response = {"Error": {"Code": code}}


class FakeECS:
    def __init__(self, running, in_flight, active, images_by_rev):
        self.running, self.in_flight, self.active, self.images_by_rev = running, in_flight, active, images_by_rev

    def describe_services(self, cluster, services):
        deployments = [{"taskDefinition": self.arn(r)} for r in (self.running, *self.in_flight)]
        return {"services": [{"taskDefinition": self.arn(self.running), "deployments": deployments}]}

    def list_task_definitions(self, familyPrefix, status, sort, maxResults):
        return {"taskDefinitionArns": [self.arn(r) for r in self.active][:maxResults]}

    def describe_task_definition(self, taskDefinition):
        rev = int(taskDefinition.rsplit(":", 1)[1])
        return {"taskDefinition": {"family": "facilitator-production", "revision": rev,
                                   "containerDefinitions": [{"image": i} for i in self.images_by_rev[rev]]}}

    @staticmethod
    def arn(rev):
        return f"arn:aws:ecs:us-east-2:ACCOUNT:task-definition/facilitator-production:{rev}"


class FakeECR:
    def __init__(self, images, preview=None, statuses=("COMPLETE",)):
        # images: digest -> {"tags": [...], "children": [...]}
        self.images = images
        self.preview = list(preview or [])
        self.statuses = list(statuses)
        self.put = []

    def _find(self, ref):
        if "imageDigest" in ref:
            return ref["imageDigest"] if ref["imageDigest"] in self.images else None
        return next((d for d, i in self.images.items() if ref["imageTag"] in i["tags"]), None)

    def describe_images(self, repositoryName, imageIds=None, filter=None, maxResults=None, nextToken=None):
        if imageIds is None:
            details = [{"imageDigest": d, "imageTags": list(i["tags"])} for d, i in self.images.items() if i["tags"]]
            half = len(details) // 2
            if nextToken is None and half:
                return {"imageDetails": details[:half], "nextToken": "p2"}
            return {"imageDetails": details[half:] if nextToken else details}
        found = self._find(imageIds[0])
        if found is None:
            raise AwsError("ImageNotFoundException")
        return {"imageDetails": [{"imageDigest": found, "imageTags": list(self.images[found]["tags"])}]}

    def batch_get_image(self, repositoryName, imageIds, acceptedMediaTypes):
        out = []
        for ref in imageIds:
            d = ref["imageDigest"]
            body = {"manifests": [{"digest": c} for c in self.images[d]["children"]]}
            out.append({"imageId": {"imageDigest": d}, "imageManifest": json.dumps(body), "imageManifestMediaType": INDEX})
        return {"images": out}

    def put_image(self, repositoryName, imageManifest, imageManifestMediaType, imageTag, imageDigest):
        self.put.append((imageDigest, imageTag))
        if imageTag in self.images[imageDigest]["tags"]:
            raise AwsError("ImageAlreadyExistsException")
        self.images[imageDigest]["tags"].append(imageTag)

    def start_lifecycle_policy_preview(self, repositoryName, lifecyclePolicyText):
        self.policy_text = lifecyclePolicyText

    def get_lifecycle_policy_preview(self, repositoryName, maxResults, nextToken=None):
        status = self.statuses.pop(0) if self.statuses else "COMPLETE"
        if status != "COMPLETE":
            return {"status": status}
        start = int(nextToken or 0)
        page = {"status": "COMPLETE", "previewResults": [{"imageDigest": d} for d in self.preview[start:start + 2]]}
        if start + 2 < len(self.preview):
            page["nextToken"] = str(start + 2)
        return page


def world(preview=(), statuses=("COMPLETE",), drop_running=False):
    images = {
        digest(1): {"tags": ["2.48.0-aaaaaaa", "latest"], "children": [digest(101), digest(102)]},
        digest(2): {"tags": ["2.47.0-bbbbbbb"], "children": [digest(201)]},
        digest(3): {"tags": ["2.46.0-ccccccc"], "children": [digest(301)]},
        digest(4): {"tags": ["2.45.0-ddddddd"], "children": [digest(401)]},
        digest(5): {"tags": ["v1.1.1"], "children": []},
        digest(6): {"tags": ["2.49.0-eeeeeee"], "children": [digest(601)]},
        digest(7): {"tags": ["2.10.0-old"], "children": [digest(701)]},
    }
    if drop_running:
        del images[digest(1)]
    ecs = FakeECS(
        running=12, in_flight=[13], active=[13, 12, 11, 10, 9],
        images_by_rev={
            13: [f"{REG}/facilitator:2.49.0-eeeeeee", f"{REG}/facilitator-otel-collector:1"],
            12: [f"{REG}/facilitator:2.48.0-aaaaaaa"],
            11: [f"{REG}/facilitator@{digest(2)}"],
            10: [f"{REG}/facilitator:2.46.0-ccccccc"],
            9: [f"{REG}/facilitator:2.45.0-ddddddd"],
        },
    )
    return ecs, FakeECR(images, preview, statuses)


def collect(ecs, ecr, last=3, extra=()):
    return anchors_mod.collect_anchors(ecs, ecr, "facilitator", "c", "s", "facilitator-production", last, list(extra))


class ParseRefTest(unittest.TestCase):
    def test_refs(self):
        p = anchors_mod.parse_ref
        self.assertEqual(p(f"{REG}/facilitator:2.48.0-abc", "facilitator"), {"imageTag": "2.48.0-abc"})
        self.assertEqual(p(f"{REG}/facilitator@{digest(9)}", "facilitator"), {"imageDigest": digest(9)})
        self.assertEqual(p(f"{REG}/facilitator:t@{digest(9)}", "facilitator"), {"imageDigest": digest(9)})
        self.assertEqual(p(f"{REG}/facilitator", "facilitator"), {"imageTag": "latest"})
        self.assertIsNone(p(f"{REG}/facilitator-grafana:1", "facilitator"))
        self.assertIsNone(p(f"{REG}/Facilitator:1", "facilitator"))
        self.assertIsNone(p("facilitator:1", "facilitator"))
        self.assertIsNone(p("", "facilitator"))


class AnchorTest(unittest.TestCase):
    def test_running_in_flight_last_revisions_and_repo_named(self):
        anchors, warnings = collect(*world())
        digests = {a["digest"] for a in anchors}
        # running 12, in-flight 13, last 3 ACTIVE (13, 12, 11), v1.1.1 from docs/ROLLBACK_PLAN.md
        self.assertEqual(digests, {digest(6), digest(1), digest(2), digest(5)})
        self.assertNotIn(digest(3), digests)
        self.assertEqual(warnings, [])
        by = {a["digest"]: a for a in anchors}
        self.assertEqual(by[digest(1)]["children"], [digest(101), digest(102)])

    def test_an_in_flight_rollback_is_an_anchor(self):
        ecs, ecr = world()
        ecs.in_flight = [9]
        anchors, _ = collect(ecs, ecr, last=2)
        self.assertIn(digest(4), {a["digest"] for a in anchors})

    def test_last_widens_the_window(self):
        anchors, _ = collect(*world(), last=5)
        self.assertLessEqual({digest(3), digest(4)}, {a["digest"] for a in anchors})

    def test_a_missing_deployed_image_is_a_stop(self):
        with self.assertRaises(anchors_mod.MissingAnchor):
            collect(*world(drop_running=True))

    def test_a_missing_repo_named_tag_is_a_warning(self):
        _, warnings = collect(*world(), extra=["v0.0.0-gone"])
        self.assertEqual(len(warnings), 1)
        self.assertIn("v0.0.0-gone", warnings[0])

    def test_protect_tags_by_digest_once(self):
        ecs, ecr = world()
        anchors, _ = collect(ecs, ecr)
        added = anchors_mod.protect(ecr, "facilitator", anchors)
        self.assertIn("stable-2.48.0-aaaaaaa", added)
        self.assertIn("stable-v1.1.1", added)
        self.assertTrue(all(anchors_mod.protected(a) for a in anchors))
        self.assertTrue(all(d in ecr.images for d, _ in ecr.put))
        again, _ = collect(ecs, ecr)
        self.assertEqual(anchors_mod.protect(ecr, "facilitator", again), [])

    def test_anchor_tag_shapes(self):
        tag = anchors_mod.anchor_tag
        self.assertEqual(tag({"digest": digest(10), "tags": ["latest"]}), "stable-" + f"{10:064x}"[:12])
        self.assertEqual(tag({"digest": digest(10), "tags": []}), "stable-" + f"{10:064x}"[:12])
        self.assertEqual(tag({"digest": digest(10), "tags": ["a/b c"]}), "stable-a-b-c")
        self.assertLessEqual(len(tag({"digest": digest(10), "tags": ["x" * 300]})), 128)
        self.assertFalse(anchors_mod.protected({"tags": ["Stable-1", "unstable-1", " stable-1"]}))


class GateTest(unittest.TestCase):
    def run_main(self, ecr_world, *args):
        return anchors_mod.main(["--last", "3", *args], clients=ecr_world)

    def test_clean_preview_is_a_go(self):
        ecs, ecr = world(preview=[digest(7), digest(701), digest(4), digest(401)], statuses=("IN_PROGRESS", "COMPLETE"))
        with patch.object(anchors_mod.time, "sleep") as slept:
            self.assertEqual(self.run_main((ecs, ecr), "--tag", "--preview"), 0)
        slept.assert_called_once_with(5)

    def test_unprotected_anchor_is_a_stop(self):
        self.assertEqual(self.run_main(world()), 1)
        self.assertEqual(self.run_main(world(), "--preview"), 1)

    def test_expiring_anchor_child_is_a_stop(self):
        ecs, ecr = world(preview=[digest(7), digest(102)])
        self.assertEqual(self.run_main((ecs, ecr), "--tag", "--preview"), 1)

    def test_expiring_child_of_any_kept_image_is_a_stop(self):
        ecs, ecr = world(preview=[digest(401)])
        self.assertEqual(self.run_main((ecs, ecr), "--tag", "--preview"), 1)

    def test_expiring_anchor_is_a_stop(self):
        ecs, ecr = world(preview=[digest(5)])
        self.assertEqual(self.run_main((ecs, ecr), "--tag", "--preview"), 1)

    def test_failed_preview_is_a_stop(self):
        self.assertEqual(self.run_main(world(statuses=("FAILED",)), "--tag", "--preview"), 2)

    def test_missing_running_image_is_a_stop(self):
        self.assertEqual(self.run_main(world(drop_running=True)), 2)

    def test_preview_pages_are_all_read(self):
        ecs, ecr = world(preview=[digest(7), digest(701), digest(4), digest(401), digest(3)])
        self.assertEqual(anchors_mod.preview_expired(ecr, "facilitator", "{}", sleep=lambda _s: None),
                         {digest(7), digest(701), digest(4), digest(401), digest(3)})


class PolicyTest(unittest.TestCase):
    def setUp(self):
        self.rules = json.loads((TF / "ecr-facilitator-lifecycle.json").read_text(encoding="utf-8"))["rules"]

    def test_anchors_rule_comes_first_and_never_bites(self):
        first = min(self.rules, key=lambda r: r["rulePriority"])
        self.assertEqual(first["selection"]["tagStatus"], "tagged")
        self.assertEqual(first["selection"]["tagPrefixList"], [anchors_mod.ANCHOR_PREFIX])
        self.assertEqual(first["selection"]["countType"], "imageCountMoreThan")
        self.assertGreaterEqual(first["selection"]["countNumber"], 9999)

    def test_priorities_unique_and_any_last(self):
        priorities = [r["rulePriority"] for r in self.rules]
        self.assertEqual(len(priorities), len(set(priorities)))
        last = max(self.rules, key=lambda r: r["rulePriority"])
        self.assertEqual(last["selection"]["tagStatus"], "any")
        self.assertEqual([r for r in self.rules if r["selection"]["tagStatus"] == "any"], [last])
        self.assertGreaterEqual(last["selection"]["countNumber"], 30)

    def test_untagged_rule_waits_a_week(self):
        untagged = [r for r in self.rules if r["selection"]["tagStatus"] == "untagged"]
        for rule in untagged:
            self.assertEqual(rule["selection"]["countType"], "sinceImagePushed")
            self.assertGreaterEqual(rule["selection"]["countNumber"], 7)

    def test_terraform_ships_it_off(self):
        tf = (TF / "ecr-lifecycle.tf").read_text(encoding="utf-8")
        self.assertIn('count      = var.enable_facilitator_ecr_lifecycle ? 1 : 0', tf)
        self.assertIn('file("${path.module}/ecr-facilitator-lifecycle.json")', tf)
        variables = (TF / "variables.tf").read_text(encoding="utf-8")
        block = re.search(r'variable "enable_facilitator_ecr_lifecycle" \{(.*?)^\}', variables, re.S | re.M).group(1)
        self.assertRegex(block, r"default\s*=\s*false")
        tfvars = (TF / "production.auto.tfvars").read_text(encoding="utf-8")
        self.assertRegex(tfvars, r"(?m)^enable_facilitator_ecr_lifecycle\s*=\s*false")


if __name__ == "__main__":
    unittest.main()
