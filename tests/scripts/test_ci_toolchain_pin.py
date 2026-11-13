"""Guards on the Rust toolchain pin (X402-TOOLCHAIN-PIN, 2026-11-13).

rust-toolchain.toml said `channel = "stable"` and the CI action was
dtolnay/rust-toolchain@stable. On 2026-11-13 stable was 1.100.0, and release
run 43338805993 could not compile the commit that run 43151660231 had built
green on 1.99.1 the day before: algonaut_client 0.4.2 fails with E0277 under
the never-type fallback change (rust-lang/rust#148922). Nothing in the
repository had changed; the compiler had.

So rust-toolchain.toml names an exact version, and every other place that
picks a compiler names the same one:

  - the toolchain action of every workflow job that runs cargo, before its
    first cargo step (its @ref, and its `toolchain` input if it takes one);
  - every Rust base image in the Dockerfiles, whose Debian suite must stay the
    runtime stage's (the Dockerfile's own note: libssl pairs the two stages).

The file is what rustup obeys, over the runner's `rustup default` and over the
image's own toolchain (both measured on 2026-11-13); the other two only decide
whether a second compiler gets downloaded next to it. What does beat the file
is RUSTUP_TOOLCHAIN and an explicit `+toolchain` (measured too), so no
workflow may set or pass either, nor run rustup to pick or fetch another one.

Offline and without credentials: they read the files as text, and the
workflows through the same PyYAML loader as scripts/ci_paths_selftest.py.

Run:  python3 -m unittest discover -s tests/scripts -p 'test_ci_*.py'
"""

from __future__ import annotations

import re
import sys
import tomllib
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import ci_paths_selftest  # noqa: E402

TOOLCHAIN_FILE = REPO / "rust-toolchain.toml"
DOCKERFILES = [REPO / "Dockerfile", *sorted((REPO / "docker").glob("Dockerfile*"))]

# rustup's exact form: three numbers. `stable`, `beta` and `nightly` move with
# the release train, and so does a two-number `1.99` (the newest 1.99.x).
EXACT = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")

TOOLCHAIN_ACTION = "dtolnay/rust-toolchain"
# Actions with "rust" in their name that install no compiler.
NOT_INSTALLERS = {"Swatinem/rust-cache"}

# Shell that picks or fetches a compiler other than the file's, or overrides it.
OVERRIDE = re.compile(
    r"\brustup\s+(?:default|override|update|install|run|toolchain\s+install)\b"
    r"|\b(?:cargo|rustc)\s+\+"
    r"|\bRUSTUP_TOOLCHAIN\b",
    re.I,
)
CARGO = re.compile(r"\bcargo\s")

# A Dockerfile FROM, its flags (--platform=...) skipped.
FROM = re.compile(r"^[ \t]*FROM[ \t]+(?:--\S+[ \t]+)*(?P<image>\S+)", re.I | re.M)


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8").replace("\r\n", "\n")


def channel() -> str:
    return tomllib.loads(read(TOOLCHAIN_FILE))["toolchain"]["channel"]


def image_parts(image: str) -> tuple[str, str]:
    """('rust', '1.99.1-bookworm') for rust:1.99.1-bookworm, docker.io/library/rust:...
    or rust:...@sha256:...; the tag is '' when there is none."""
    name = image.split("@", 1)[0]
    last = name.rsplit("/", 1)[-1]
    repo, _, tag = last.partition(":")
    return repo, tag


def from_images() -> list[tuple[Path, str]]:
    return [(path, m.group("image")) for path in DOCKERFILES if path.exists()
            for m in FROM.finditer(read(path))]


def steps():
    """(workflow, job, index, step) for every step of every workflow job."""
    for wf_name, wf in ci_paths_selftest.load_workflows().items():
        for job_name, job in (wf.get("jobs") or {}).items():
            for index, step in enumerate(job.get("steps") or []):
                yield wf_name, job_name, index, step


def env_blocks():
    """(where, env) for every env: block -- workflow, job and step."""
    for wf_name, wf in ci_paths_selftest.load_workflows().items():
        yield wf_name, wf.get("env") or {}
        for job_name, job in (wf.get("jobs") or {}).items():
            yield f"{wf_name}:{job_name}", job.get("env") or {}
            for index, step in enumerate(job.get("steps") or []):
                yield f"{wf_name}:{job_name}[{index}]", step.get("env") or {}


class TheFilePinsAnExactVersion(unittest.TestCase):
    def test_the_channel_is_an_exact_version(self):
        value = channel()
        self.assertIsInstance(value, str)
        self.assertRegex(value, rf"^{EXACT.pattern}$",
                         f"rust-toolchain.toml channel {value!r} floats; name an exact X.Y.Z")

    def test_the_toolchain_is_a_channel_not_a_path(self):
        # `path` points rustup at a local toolchain directory: nothing pins what is in it.
        self.assertNotIn("path", tomllib.loads(read(TOOLCHAIN_FILE))["toolchain"])


class EveryWorkflowInstallsTheFilesVersion(unittest.TestCase):
    def test_every_toolchain_action_names_the_files_version(self):
        pinned = channel()
        found = 0
        for wf, job, index, step in steps():
            uses = str(step.get("uses", ""))
            if not uses.startswith(TOOLCHAIN_ACTION + "@"):
                continue
            found += 1
            where = f"{wf}:{job}[{index}]"
            self.assertEqual(uses.split("@", 1)[1], pinned,
                             f"{where} installs {uses}, rust-toolchain.toml pins {pinned}")
            toolchain = (step.get("with") or {}).get("toolchain")
            if toolchain is not None:
                self.assertEqual(str(toolchain), pinned, f"{where} passes toolchain: {toolchain}")
        self.assertGreaterEqual(found, 1, f"no workflow step uses {TOOLCHAIN_ACTION}")

    def test_every_job_that_runs_cargo_installs_the_pin_first(self):
        compiling = 0
        for wf_name, wf in ci_paths_selftest.load_workflows().items():
            for job_name, job in (wf.get("jobs") or {}).items():
                job_steps = job.get("steps") or []
                cargo = [i for i, s in enumerate(job_steps) if CARGO.search(str(s.get("run", "")))]
                if not cargo:
                    continue
                compiling += 1
                action = [i for i, s in enumerate(job_steps)
                          if str(s.get("uses", "")).startswith(TOOLCHAIN_ACTION + "@")]
                self.assertTrue(action and action[0] < cargo[0],
                                f"{wf_name}:{job_name} runs cargo without installing the pinned toolchain first")
        self.assertGreaterEqual(compiling, 1, "no workflow job runs cargo: this guard checks nothing")

    def test_no_other_action_installs_rust(self):
        for wf, job, index, step in steps():
            uses = str(step.get("uses", ""))
            action = uses.split("@", 1)[0]
            if "rust" in action.lower() and action not in NOT_INSTALLERS | {TOOLCHAIN_ACTION}:
                self.fail(f"{wf}:{job}[{index}] uses {uses}: teach this guard what it installs")

    def test_no_step_overrides_or_fetches_another_toolchain(self):
        for wf, job, index, step in steps():
            run = str(step.get("run", ""))
            match = OVERRIDE.search(run)
            self.assertIsNone(match, f"{wf}:{job}[{index}] runs {match and match.group(0)!r}: "
                                     "rust-toolchain.toml decides the compiler")

    def test_no_env_block_sets_rustup_toolchain(self):
        for where, env in env_blocks():
            self.assertFalse(any(str(key).upper() == "RUSTUP_TOOLCHAIN" for key in env),
                             f"{where} sets RUSTUP_TOOLCHAIN, which beats rust-toolchain.toml")


class TheImageBuildsWithTheFilesVersion(unittest.TestCase):
    def test_every_rust_base_image_names_the_files_version(self):
        pinned = channel()
        rust = [(path, image) for path, image in from_images() if image_parts(image)[0] == "rust"]
        self.assertGreaterEqual(len(rust), 1, "no Dockerfile builds FROM rust: this guard checks nothing")
        for path, image in rust:
            tag = image_parts(image)[1]
            self.assertRegex(tag, rf"^{re.escape(pinned)}-[a-z]+$",
                             f"{path.name}: FROM {image} -- name {pinned}-<debian suite>")

    def test_the_builder_and_runtime_stages_share_a_debian_suite(self):
        images = from_images()
        builder = {image_parts(i)[1].split("-", 1)[1] for _, i in images
                   if image_parts(i)[0] == "rust" and "-" in image_parts(i)[1]}
        runtime = {image_parts(i)[1].split("-", 1)[0] for p, i in images
                   if p.name == "Dockerfile" and image_parts(i)[0] == "debian"}
        self.assertEqual(len(builder), 1, f"rust base images on several suites: {builder}")
        self.assertEqual(builder, runtime, "the builder's suite and the runtime's must move together")


if __name__ == "__main__":
    unittest.main()
