#!/usr/bin/env python3
"""Rollback anchors of the ECR `facilitator` repository, checked before any lifecycle expiry.

An ECR expiry cannot be undone, so the lifecycle policy in
terraform/environments/production/ecr-facilitator-lifecycle.json ships behind
`enable_facilitator_ecr_lifecycle = false` (COSTO-X402 B15). Before flipping it, an
operator with ECR/ECS read and ecr:PutImage runs, in this order:

    python3 scripts/ecr_rollback_anchors.py              # list the anchors, change nothing
    python3 scripts/ecr_rollback_anchors.py --tag        # add a stable-* tag to every anchor
    python3 scripts/ecr_rollback_anchors.py --preview    # ECR's own dry run of the policy

An anchor is the image of the task definition the service runs, of every in-flight
deployment, of the last --last ACTIVE revisions of the family, and every tag the repo
names as its rollback image (REPO_NAMED_ROLLBACK_TAGS). `--tag` re-tags each one, by
digest, with `stable-<tag>`, which rule 1 of the policy never expires. `--preview` asks
ECR which images the policy WOULD expire and exits 1 if that set holds an anchor, a
child manifest of an anchor, or a child manifest of any tagged image it keeps (CI pushes
with buildx, whose provenance attestation makes every release an index whose children
are untagged). Exit 0 from `--preview` is the go for the flag; anything else is a stop.

Image URIs carry the account id, so only tags and digests are ever printed.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import time
from pathlib import Path

POLICY = Path(__file__).resolve().parents[1] / "terraform/environments/production/ecr-facilitator-lifecycle.json"
ANCHOR_PREFIX = "stable-"
# docs/ROLLBACK_PLAN.md names this image as the rollback target ("Previous image").
REPO_NAMED_ROLLBACK_TAGS = ("v1.1.1",)
INDEX_TYPES = (
    "application/vnd.oci.image.index.v1+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
)
MANIFEST_TYPES = INDEX_TYPES + (
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.docker.distribution.manifest.v2+json",
)


class MissingAnchor(Exception):
    """A deployed image is not in the repository any more."""


class PreviewFailed(Exception):
    """ECR could not finish the lifecycle preview."""


def error_code(exc: Exception) -> str:
    return getattr(exc, "response", {}).get("Error", {}).get("Code", "")


def parse_ref(image: str, repository: str) -> dict | None:
    """The ECR image id of a container image string, or None if it is another repository's."""
    if "/" not in image:
        return None
    name = image.rsplit("/", 1)[1]
    name, _, digest = name.partition("@")
    repo, _, tag = name.partition(":")
    if repo != repository:
        return None
    if digest:
        return {"imageDigest": digest}
    return {"imageTag": tag or "latest"}


def deployed_refs(ecs, repository: str, cluster: str, service: str, family: str, last: int) -> list[tuple[str, dict]]:
    """(where it comes from, image id) for every image the service runs or can roll back to."""
    arns: list[str] = []
    services = ecs.describe_services(cluster=cluster, services=[service])["services"]
    for svc in services:
        arns.append(svc["taskDefinition"])
        arns.extend(d["taskDefinition"] for d in svc.get("deployments", []))
    listed = ecs.list_task_definitions(familyPrefix=family, status="ACTIVE", sort="DESC", maxResults=max(1, min(last, 100)))
    arns.extend(listed["taskDefinitionArns"][:last])
    refs: list[tuple[str, dict]] = []
    seen: set[str] = set()
    for arn in dict.fromkeys(arns):
        definition = ecs.describe_task_definition(taskDefinition=arn)["taskDefinition"]
        revision = f"{definition.get('family', family)}:{definition.get('revision', '?')}"
        for container in definition.get("containerDefinitions", []):
            ref = parse_ref(container.get("image", ""), repository)
            if ref is None:
                continue
            key = json.dumps(ref, sort_keys=True)
            if key not in seen:
                seen.add(key)
                refs.append((f"task definition {revision}", ref))
    return refs



def describe(ecr, repository: str, ref: dict) -> dict | None:
    try:
        details = ecr.describe_images(repositoryName=repository, imageIds=[ref])["imageDetails"]
    except Exception as exc:  # botocore ClientError, or a test double of it
        if error_code(exc) == "ImageNotFoundException":
            return None
        raise
    return details[0] if details else None


def manifest(ecr, repository: str, digest: str) -> tuple[str, str]:
    images = ecr.batch_get_image(
        repositoryName=repository,
        imageIds=[{"imageDigest": digest}],
        acceptedMediaTypes=list(MANIFEST_TYPES),
    )["images"]
    if not images:
        raise MissingAnchor(f"{digest}: no manifest")
    return images[0]["imageManifest"], images[0]["imageManifestMediaType"]


def children(manifest_text: str) -> list[str]:
    return [m["digest"] for m in json.loads(manifest_text).get("manifests", [])]


def collect_anchors(ecs, ecr, repository: str, cluster: str, service: str, family: str, last: int, extra: list[str]):
    """Anchors as dicts (digest, tags, sources, children) plus warnings for missing repo-named tags."""
    wanted = deployed_refs(ecs, repository, cluster, service, family, last)
    wanted += [("named by the repo as rollback", {"imageTag": tag}) for tag in (*REPO_NAMED_ROLLBACK_TAGS, *extra)]
    anchors: dict[str, dict] = {}
    warnings: list[str] = []
    for source, ref in wanted:
        detail = describe(ecr, repository, ref)
        label = ref.get("imageTag") or ref.get("imageDigest")
        if detail is None:
            if source.startswith("task definition"):
                raise MissingAnchor(f"{label} ({source}) is not in {repository}")
            warnings.append(f"{label} ({source}) is not in {repository}; nothing to protect")
            continue
        digest = detail["imageDigest"]
        anchor = anchors.setdefault(digest, {"digest": digest, "tags": list(detail.get("imageTags", [])), "sources": [], "children": []})
        anchor["sources"].append(f"{label} <- {source}")
    for anchor in anchors.values():
        text, _ = manifest(ecr, repository, anchor["digest"])
        anchor["children"] = children(text)
    return list(anchors.values()), warnings


def protected(anchor: dict) -> bool:
    return any(tag.startswith(ANCHOR_PREFIX) for tag in anchor["tags"])


def anchor_tag(anchor: dict) -> str:
    plain = [t for t in anchor["tags"] if not t.startswith(ANCHOR_PREFIX) and t != "latest"]
    base = plain[0] if plain else anchor["digest"].split(":", 1)[-1][:12]
    return (ANCHOR_PREFIX + re.sub(r"[^A-Za-z0-9._-]", "-", base))[:128]


def protect(ecr, repository: str, anchors: list[dict]) -> list[str]:
    """Tag every unprotected anchor stable-*, by digest. Returns the tags added."""
    added = []
    for anchor in anchors:
        if protected(anchor):
            continue
        text, media_type = manifest(ecr, repository, anchor["digest"])
        tag = anchor_tag(anchor)
        try:
            ecr.put_image(
                repositoryName=repository,
                imageManifest=text,
                imageManifestMediaType=media_type,
                imageTag=tag,
                imageDigest=anchor["digest"],
            )
        except Exception as exc:
            if error_code(exc) != "ImageAlreadyExistsException":
                raise
        anchor["tags"].append(tag)
        added.append(tag)
    return added


def preview_expired(ecr, repository: str, policy_text: str, sleep=None, polls: int = 120) -> set[str]:
    """Digests ECR's lifecycle preview would expire under `policy_text`."""
    ecr.start_lifecycle_policy_preview(repositoryName=repository, lifecyclePolicyText=policy_text)
    for _ in range(polls):
        page = ecr.get_lifecycle_policy_preview(repositoryName=repository, maxResults=100)
        status = page.get("status")
        if status == "COMPLETE":
            break
        if status != "IN_PROGRESS":
            raise PreviewFailed(f"lifecycle preview ended {status}")
        (sleep or time.sleep)(5)
    else:
        raise PreviewFailed("lifecycle preview did not finish")
    expired = set()
    while True:
        expired.update(r["imageDigest"] for r in page.get("previewResults", []))
        token = page.get("nextToken")
        if not token:
            return expired
        page = ecr.get_lifecycle_policy_preview(repositoryName=repository, maxResults=100, nextToken=token)


def kept_tagged_indexes(ecr, repository: str, expired: set[str]) -> dict[str, list[str]]:
    """digest -> children, for every tagged image the policy keeps."""
    digests = []
    kwargs = {"repositoryName": repository, "filter": {"tagStatus": "TAGGED"}, "maxResults": 1000}
    while True:
        page = ecr.describe_images(**kwargs)
        digests += [d["imageDigest"] for d in page["imageDetails"] if d["imageDigest"] not in expired]
        if not page.get("nextToken"):
            break
        kwargs["nextToken"] = page["nextToken"]
    kept = {}
    for start in range(0, len(digests), 100):
        batch = ecr.batch_get_image(
            repositoryName=repository,
            imageIds=[{"imageDigest": d} for d in digests[start:start + 100]],
            acceptedMediaTypes=list(MANIFEST_TYPES),
        )["images"]
        for image in batch:
            kept[image["imageId"]["imageDigest"]] = children(image["imageManifest"])
    return kept


def gate(anchors: list[dict], expired: set[str], kept: dict[str, list[str]]) -> list[str]:
    """Every reason the policy must not be applied. Empty means go."""
    problems = []
    for anchor in anchors:
        if anchor["digest"] in expired:
            problems.append(f"anchor {anchor['digest']} ({'; '.join(anchor['sources'])}) would expire")
        for child in anchor["children"]:
            if child in expired:
                problems.append(f"child {child} of anchor {anchor['digest']} would expire")
    for digest, kids in kept.items():
        for child in kids:
            if child in expired:
                problems.append(f"child {child} of kept image {digest} would expire")
    return sorted(set(problems))


def main(argv: list[str] | None = None, clients=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--region", default="us-east-2")
    parser.add_argument("--repository", default="facilitator")
    parser.add_argument("--cluster", default="facilitator-production")
    parser.add_argument("--service", default="facilitator-production")
    parser.add_argument("--family", default="facilitator-production")
    parser.add_argument("--last", type=int, default=10, help="ACTIVE task definition revisions to keep as anchors")
    parser.add_argument("--extra-tag", action="append", default=[], help="another rollback tag to protect")
    parser.add_argument("--tag", action="store_true", help="add a stable-* tag to every unprotected anchor")
    parser.add_argument("--preview", action="store_true", help="run ECR's lifecycle preview and gate on it")
    parser.add_argument("--policy", type=Path, default=POLICY)
    args = parser.parse_args(argv)
    if args.last < 1:
        parser.error("--last must be at least 1")

    if clients is None:
        import boto3

        clients = (boto3.client("ecs", region_name=args.region), boto3.client("ecr", region_name=args.region))
    ecs, ecr = clients
    try:
        anchors, warnings = collect_anchors(ecs, ecr, args.repository, args.cluster, args.service, args.family, args.last, args.extra_tag)
    except MissingAnchor as exc:
        print(f"[FAIL] {exc}")
        return 2
    for warning in warnings:
        print(f"[WARN] {warning}")
    if args.tag:
        for tag in protect(ecr, args.repository, anchors):
            print(f"[OK] tagged {tag}")
    for anchor in anchors:
        state = "protected" if protected(anchor) else "UNPROTECTED"
        print(f"{anchor['digest']}  {state}  tags={','.join(anchor['tags'])}  from={'; '.join(anchor['sources'])}")
    unprotected = [a for a in anchors if not protected(a)]
    if not args.preview:
        return 1 if unprotected else 0
    try:
        expired = preview_expired(ecr, args.repository, args.policy.read_text(encoding="utf-8"))
    except PreviewFailed as exc:
        print(f"[FAIL] {exc}")
        return 2
    problems = gate(anchors, expired, kept_tagged_indexes(ecr, args.repository, expired))
    problems += [f"anchor {a['digest']} has no {ANCHOR_PREFIX}* tag" for a in unprotected]
    for problem in problems:
        print(f"[FAIL] {problem}")
    print(f"preview: {len(expired)} images would expire, {len(anchors)} anchors checked")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
