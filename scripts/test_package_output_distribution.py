"""Local pair publication: validate actual tar bytes, rollback and interrupted runs."""
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import package_output as output

BASE = "herdr-app-macos-x86_64"
MEMBER = "Herdr.app/Contents/MacOS/Herdr"


def pair(folder, content):
    folder.mkdir(exist_ok=True)
    archive = folder / (BASE + ".tar.gz")
    with tarfile.open(archive, "w:gz") as tar:
        item = tarfile.TarInfo(MEMBER)
        item.size, item.mode = len(content), 0o755
        tar.addfile(item, io.BytesIO(content))
    receipt = folder / (BASE + ".build.json")
    receipt.write_text(json.dumps({"archive_member": MEMBER, "archive_sha256": output.digest(archive),
                                  "packaged_executable_sha256": hashlib.sha256(content).hexdigest()}))
    return archive, receipt


class OutputTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="herdr-output-pair-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.dist = self.root / "dist"
        self.dist.mkdir()
        self.lock = self.dist / ("." + BASE + ".lock")
        self.lock.mkdir()
        pair(self.dist, b"previous executable")
        self.previous = {p.name: p.read_bytes() for p in self.dist.iterdir() if p.is_file()}
        self.archive, self.receipt = pair(self.root / "new", b"new executable")

    def publish(self):
        output.publish(self.archive, self.receipt, self.dist, self.lock)

    def previous_unchanged(self):
        for name, data in self.previous.items():
            self.assertEqual((self.dist / name).read_bytes(), data)
        self.assertFalse((self.lock / "transaction.json").exists())
        self.assertEqual(list(self.dist.glob(".*.publish-*")), [])

    def test_success_keeps_compatible_names_and_matching_actual_member(self):
        self.publish()
        output.verify(self.dist / self.archive.name, self.dist / self.receipt.name)
        self.assertEqual((self.dist / self.archive.name).read_bytes(), self.archive.read_bytes())
        self.assertEqual((self.dist / self.receipt.name).read_bytes(), self.receipt.read_bytes())
        self.assertFalse((self.lock / "transaction.json").exists())

    def test_receipt_for_another_payload_is_rejected_before_replacement(self):
        data = json.loads(self.receipt.read_text())
        data["packaged_executable_sha256"] = "0" * 64
        self.receipt.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "archived executable"):
            self.publish()
        self.previous_unchanged()

    def test_failure_of_either_final_move_restores_the_previous_pair(self):
        replace = os.replace
        for failed in (self.archive.name, self.receipt.name):
            with self.subTest(failed=failed):
                def failing(src, dest):
                    if Path(src).name == failed and Path(dest).parent == self.dist:
                        raise OSError("injected final move failure")
                    replace(src, dest)
                with patch.object(output.os, "replace", failing), self.assertRaisesRegex(OSError, "final move"):
                    self.publish()
                self.previous_unchanged()

    def test_copy_failure_never_touches_previous_outputs(self):
        copy = shutil.copy2
        def failing(src, dest):
            if Path(src) == self.receipt:
                raise OSError("injected copy failure")
            return copy(src, dest)
        with patch.object(output.shutil, "copy2", failing), self.assertRaisesRegex(OSError, "copy failure"):
            self.publish()
        self.previous_unchanged()

    def test_initial_publication_failure_removes_its_partial_new_pair(self):
        for name in self.previous:
            (self.dist / name).unlink()
        replace = os.replace
        def failing(src, dest):
            if Path(src).name == self.receipt.name:
                raise OSError("receipt move failed")
            replace(src, dest)
        with patch.object(output.os, "replace", failing), self.assertRaises(OSError):
            self.publish()
        self.assertFalse((self.dist / self.archive.name).exists())
        self.assertFalse((self.dist / self.receipt.name).exists())
        self.assertFalse((self.lock / "transaction.json").exists())

    def test_interrupt_immediately_after_rename_is_recoverable(self):
        replace = os.replace
        def interrupted(src, dest):
            replace(src, dest)
            if Path(src).name == self.archive.name:
                raise InterruptedError("interrupted after rename, before returning")
        with patch.object(output.os, "replace", interrupted), self.assertRaises(InterruptedError):
            self.publish()
        self.previous_unchanged()

    def test_rollback_failure_keeps_backups_and_blocks_another_publication(self):
        replace = os.replace
        def failing(src, dest):
            if Path(src).name in (self.receipt.name, "previous-archive"):
                raise OSError("injected receipt/restore failure")
            replace(src, dest)
        with patch.object(output.os, "replace", failing), self.assertRaisesRegex(RuntimeError, "recovery required"):
            self.publish()
        marker = self.lock / "transaction.json"
        stage = Path(json.loads(marker.read_text())["stage"])
        self.assertEqual((stage / "previous-archive").read_bytes(), self.previous[self.archive.name])
        with self.assertRaisesRegex(ValueError, "unfinished publication"):
            self.publish()

    def test_uncatchable_child_exit_retains_recovery_marker_and_previous_bytes(self):
        code = '''import os,sys,pathlib
sys.path.insert(0,sys.argv[1])
import package_output as p
replace=os.replace
def fail(src,dest):
 replace(src,dest)
 if pathlib.Path(src).name.endswith('.tar.gz'):os._exit(86)
p.os.replace=fail
p.publish(*sys.argv[2:])
'''
        run = subprocess.run([sys.executable, "-B", "-c", code, str(Path(output.__file__).parent),
                              str(self.archive), str(self.receipt), str(self.dist), str(self.lock)],
                             capture_output=True, text=True)
        self.assertEqual(run.returncode, 86, run.stderr)
        stage = Path(json.loads((self.lock / "transaction.json").read_text())["stage"])
        self.assertEqual((stage / "previous-archive").read_bytes(), self.previous[self.archive.name])
        self.assertEqual((stage / "previous-receipt").read_bytes(), self.previous[self.receipt.name])
        self.assertTrue(self.lock.is_dir())


if __name__ == "__main__":
    unittest.main()
