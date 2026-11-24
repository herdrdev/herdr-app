"""Protect required native release coverage and the actual manifest workflow step."""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest

from release_manifest import manifest, TARGETS

ROOT = Path(__file__).resolve().parent.parent


def job(workflow, name):
    match = re.search(r"^  " + re.escape(name) + r":\n(.*?)(?=^  [\w-]+:\n|\Z)", workflow, re.M | re.S)
    if not match:
        raise ValueError(f"missing job {name}")
    return match.group(1)


def check_matrices(ci, release):
    for workflow, name, key, required in (
        (ci, "platform-tests", "platform", {"linux-x86_64", "macos-aarch64", "macos-x86_64", "windows-x86_64"}),
        (release, "build", "target", set(TARGETS)),
        (release, "mac-dmg", "target", {"macos-aarch64", "macos-x86_64"}),
    ):
        block = job(workflow, name).split("matrix:\n", 1)[1].split("\n    runs-on:", 1)[0]
        rows = re.findall(r"^\s*- \{ " + key + r": ([\w-]+), runner: ([\w.-]+) \}\s*$", block, re.M)
        values = dict(rows)
        if len(values) != len(rows) or not required.issubset(values):
            raise ValueError(f"{name}: missing required platforms {sorted(required - values.keys())} or duplicate matrix entry")
        runner = values["macos-x86_64"]
        if not runner.startswith("macos-") or not runner.endswith("-intel"):
            raise ValueError(f"{name}: Intel target needs a native Intel macOS runner")


class WorkflowTests(unittest.TestCase):
    def workflows(self):
        return tuple((ROOT / ".github/workflows" / name).read_text() for name in ("ci.yml", "release.yml"))

    def test_actual_workflows_keep_all_required_platforms(self):
        check_matrices(*self.workflows())

    def test_removing_intel_from_each_of_the_three_matrices_is_rejected(self):
        ci, release = self.workflows()
        for name, which in (("platform-tests", "ci"), ("build", "release"), ("mac-dmg", "release")):
            with self.subTest(job=name):
                text = ci if which == "ci" else release
                before = job(text, name)
                after, count = re.subn(r"^.*- \{ (?:platform|target): macos-x86_64, runner: .*\}\n", "", before, flags=re.M)
                self.assertEqual(count, 1)
                changed = text.replace(before, after, 1)
                with self.assertRaisesRegex(ValueError, name):
                    check_matrices(changed if which == "ci" else ci, changed if which == "release" else release)

    def test_arm_runner_cannot_silently_replace_an_intel_runner(self):
        ci, release = self.workflows()
        with self.assertRaisesRegex(ValueError, "native Intel"):
            check_matrices(ci.replace("runner: macos-15-intel", "runner: macos-15"), release)

    def test_actual_manifest_step_rejects_a_missing_intel_archive(self):
        _, release = self.workflows()
        block = job(release, "publish").split("      - name: Manifest\n        run: |\n", 1)[1].split("      - name:", 1)[0]
        script = textwrap.dedent(block)
        with tempfile.TemporaryDirectory(prefix="herdr-release-manifest-") as tmp:
            root = Path(tmp)
            (root / "dist").mkdir(); (root / "scripts").mkdir(); (root / "distribution").mkdir()
            (root / "Cargo.toml").write_text('version = "1.2.3"\n')
            (root / "distribution/core-version").write_text("0.1.11\n")
            for name in ("release_manifest.py", "dmg_manifest.py"):
                shutil.copy2(ROOT / "scripts" / name, root / "scripts" / name)
            for target in TARGETS:
                if target != "macos-x86_64":
                    (root / f"dist/herdr-app-{target}.tar.gz").write_bytes(target.encode())
            env = {"HOME":tmp,"PATH":f"{Path(sys.executable).parent}:/usr/bin:/bin", "RELEASES_URL":"https://releases.example", "PYTHONDONTWRITEBYTECODE":"1"}
            bad = subprocess.run(["bash", "-euo", "pipefail", "-c", script], cwd=root, env=env, capture_output=True, text=True)
            self.assertNotEqual(bad.returncode, 0)
            self.assertIn("macos-x86_64", bad.stderr)
            (root / "dist/herdr-app-macos-x86_64.tar.gz").write_bytes(b"Intel artifact")
            good = subprocess.run(["bash", "-euo", "pipefail", "-c", script], cwd=root, env=env, capture_output=True, text=True)
            self.assertEqual(good.returncode, 0, good.stderr)
            doc = json.loads((root / "dist/manifest.json").read_bytes())
            self.assertEqual(set(doc["targets"]), set(TARGETS))
            self.assertEqual(doc["targets"]["macos-x86_64"]["sha256"], hashlib.sha256(b"Intel artifact").hexdigest())


class TarballManifestTests(unittest.TestCase):
    def test_every_required_archive_must_be_regular_nonempty_and_present(self):
        with tempfile.TemporaryDirectory(prefix="herdr-tarball-manifest-") as tmp:
            root = Path(tmp)
            for target in TARGETS:
                (root / f"herdr-app-{target}.tar.gz").write_bytes(target.encode())
            self.assertEqual(set(manifest("1.2.3", "0.1.11", root, "https://releases.example")["targets"]), set(TARGETS))
            for target in TARGETS:
                path = root / f"herdr-app-{target}.tar.gz"
                for state in ("missing", "empty", "symlink"):
                    with self.subTest(target=target, state=state):
                        path.unlink()
                        if state == "empty":path.touch()
                        if state == "symlink":path.symlink_to(root / "outside")
                        with self.assertRaises(ValueError):manifest("1.2.3", "0.1.11", root, "https://releases.example")
                        path.unlink(missing_ok=True);path.write_bytes(target.encode())


if __name__ == "__main__":
    unittest.main()
