"""Concurrent publication tests against an atomic in-memory S3 fixture.

No credentials, network, app installation or production bucket access. R2's
documented conditional PutObject semantics are the fixture's storage contract.
"""
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import re
import subprocess
import shutil
import sqlite3
import sys
import tempfile
import threading
import unittest
from unittest import mock

from publish_dmg import Conflict, Object, Publisher, S3
from dmg_manifest import version_key

SOURCE = "a" * 40
ROOT = Path(__file__).resolve().parent.parent


class Store:
    def __init__(self):
        self.data, self.writes = {}, []
        self.lock = threading.RLock()
        self.barrier_key, self.barrier = None, None
        self.local = threading.local()
        self.fail = set()

    def get(self, key):
        with self.lock:
            item = self.data.get(key)
            return None if item is None else Object(item.body, item.etag, dict(item.metadata))

    def keys(self):
        with self.lock:
            return list(self.data)

    def put(self, key, body, metadata, content_type, etag=None):
        if key == self.barrier_key and not getattr(self.local, "waited", False):
            self.local.waited = True
            self.barrier.wait(timeout=10)
        with self.lock:
            if key in self.fail:
                raise RuntimeError("injected upload failure")
            old = self.data.get(key)
            if (etag is None and old is not None) or (etag is not None and (old is None or old.etag != etag)):
                raise Conflict("PreconditionFailed")
            # Real ETags identify content; do not rely on a test-only generation.
            self.data[key] = Object(body, '"' + hashlib.md5(body).hexdigest() + '"', dict(metadata))
            self.writes.append((key, body, etag, content_type))


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="herdr-dmg-publish-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.store = Store()

    def files(self, name, version="1.2.3", extra=""):
        directory = self.work / name
        directory.mkdir()
        for target in ("macos-aarch64", "macos-x86_64"):
            (directory / f"Herdr-{target}.dmg").write_bytes(f"signed/stapled {version} {target} {extra}".encode())
        return directory

    def publish(self, directory, version="1.2.3", source=SOURCE):
        p = Publisher(self.store, version, source)
        for target in ("macos-aarch64", "macos-x86_64"):
            p.image(target, directory / f"Herdr-{target}.dmg")
        p.release(directory, "https://releases.example")
        return p

    def assert_manifest(self, key="app/dmg/latest.json"):
        doc = json.loads(self.store.get(key).body)
        for target, item in doc["targets"].items():
            stored = self.store.get(f"app/{doc['version']}/Herdr-{target}.dmg")
            self.assertEqual(hashlib.sha256(stored.body).hexdigest(), item["sha256"])
            if target == "macos-aarch64":
                self.assertEqual(stored.body, self.store.get(f"app/{doc['version']}/Herdr.dmg").body)
        return doc

    def test_rebuilt_published_tag_reuses_committed_bytes_and_changes_no_public_object(self):
        original = self.files("first")
        self.publish(original)
        before = dict(self.store.data)
        count = len(self.store.writes)
        rebuilt = self.files("rebuilt", extra="different timestamped signature")
        self.publish(rebuilt)
        self.assertEqual(self.store.data, before)
        self.assertEqual(len(self.store.writes), count)
        for path in rebuilt.iterdir():
            self.assertEqual(path.read_bytes(), (original / path.name).read_bytes())
        self.assert_manifest()

    def test_partial_alias_failure_resumes_with_first_writer_bytes(self):
        first = self.files("first")
        p = Publisher(self.store, "1.2.3", SOURCE)
        self.store.fail.add("app/Herdr-macos-aarch64.dmg")
        with self.assertRaisesRegex(RuntimeError, "injected"):
            p.image("macos-aarch64", first / "Herdr-macos-aarch64.dmg")
        committed = self.store.get("app/1.2.3/Herdr-macos-aarch64.dmg").body
        self.assertIsNone(self.store.get("app/dmg/latest.json"))
        self.store.fail.clear()
        retry = self.files("retry", extra="regenerated")
        self.publish(retry)
        self.assertEqual((retry / "Herdr-macos-aarch64.dmg").read_bytes(), committed)
        self.assert_manifest()

    def test_concurrent_same_commit_candidates_choose_one_canonical_image(self):
        a, b = self.files("a", extra="A"), self.files("b", extra="B")
        self.store.barrier_key = "app/1.2.3/Herdr-macos-aarch64.dmg"
        self.store.barrier = threading.Barrier(2)
        def run(directory):
            Publisher(self.store, "1.2.3", SOURCE).image("macos-aarch64", directory / "Herdr-macos-aarch64.dmg")
        with ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(run, (a, b)))
        winner = self.store.get(self.store.barrier_key).body
        self.assertEqual(sum(k == self.store.barrier_key for k, *_ in self.store.writes), 1)
        for key in (self.store.barrier_key, "app/1.2.3/Herdr.dmg", "app/Herdr.dmg", "app/Herdr-macos-aarch64.dmg"):
            self.assertEqual(self.store.get(key).body, winner)
        self.assertEqual((a / "Herdr-macos-aarch64.dmg").read_bytes(), winner)
        self.assertEqual((b / "Herdr-macos-aarch64.dmg").read_bytes(), winner)

    def test_different_commit_or_unknown_provenance_cannot_replace_a_release(self):
        first = self.files("first")
        self.publish(first)
        before = dict(self.store.data)
        with self.assertRaises(RuntimeError):
            self.publish(self.files("retag", extra="retag"), source="b" * 40)
        self.assertEqual(self.store.data, before)
        old = self.store.data["app/1.2.3/Herdr-macos-aarch64.dmg"]
        old.metadata.clear()
        with self.assertRaisesRegex(RuntimeError, "provenance"):
            self.publish(self.files("unknown", extra="unknown"))

    def test_existing_legacy_versioned_arm_dmg_is_never_overwritten(self):
        self.store.put("app/1.2.3/Herdr.dmg", b"previously published ARM bytes", {}, "application/x-apple-diskimage")
        before = dict(self.store.data)
        with self.assertRaises(RuntimeError):
            self.publish(self.files("new"))
        self.assertEqual(self.store.data, before)
        self.assertIsNone(self.store.get("app/1.2.3/Herdr-macos-aarch64.dmg"))

    def test_legacy_latest_alias_migrates_only_from_verified_version_history(self):
        previous = b"known previous signed ARM image"
        self.store.put("app/1.2.2/Herdr.dmg", previous, {}, "application/x-apple-diskimage")
        self.store.put("app/Herdr.dmg", previous, {}, "application/x-apple-diskimage")
        self.publish(self.files("new"))
        self.assertEqual(self.store.get("app/Herdr.dmg").metadata["version"], "1.2.3")
        self.assertEqual(self.store.get("app/1.2.2/Herdr.dmg").body, previous)

    def test_unidentifiable_legacy_alias_is_left_unchanged(self):
        self.store.put("app/Herdr.dmg", b"unknown history", {}, "application/x-apple-diskimage")
        with self.assertRaisesRegex(RuntimeError, "identify legacy"):
            self.publish(self.files("new"))
        self.assertEqual(self.store.get("app/Herdr.dmg").body, b"unknown history")
        self.assertIsNone(self.store.get("app/dmg/latest.json"))

    def test_concurrent_different_versions_cannot_roll_aliases_back(self):
        old, a, b = self.files("old", "1.0.0"), self.files("a", "1.2.3"), self.files("b", "2.0.0")
        self.publish(old, "1.0.0")
        self.store.barrier_key = "app/Herdr-macos-x86_64.dmg"
        self.store.barrier = threading.Barrier(2)
        def run(args):
            directory, version = args
            Publisher(self.store, version, SOURCE).image("macos-x86_64", directory / "Herdr-macos-x86_64.dmg")
        with ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(run, ((a, "1.2.3"), (b, "2.0.0"))))
        self.assertEqual(self.store.get(self.store.barrier_key).metadata["version"], "2.0.0")
        self.assertEqual(self.store.get(self.store.barrier_key).body, (b / "Herdr-macos-x86_64.dmg").read_bytes())

    def test_older_tag_retries_do_not_roll_any_latest_path_back(self):
        old, new = self.files("old", "1.2.3"), self.files("new", "2.0.0")
        self.publish(old, "1.2.3")
        self.publish(new, "2.0.0")
        latest = {key: item for key, item in self.store.data.items() if key.count("/") == 1 or key == "app/dmg/latest.json"}
        self.publish(old, "1.2.3")
        for key, item in latest.items():
            self.assertEqual(self.store.get(key), item)
        self.assertEqual(self.assert_manifest()["version"], "2.0.0")

    def test_concurrent_complete_releases_cannot_roll_manifest_latest_back(self):
        a, b = self.files("a", "1.2.3"), self.files("b", "2.0.0")
        for directory, version in ((a, "1.2.3"), (b, "2.0.0")):
            p = Publisher(self.store, version, SOURCE)
            for target in ("macos-aarch64", "macos-x86_64"):
                p.image(target, directory / f"Herdr-{target}.dmg")
        self.store.barrier_key = "app/dmg/latest.json"
        self.store.barrier = threading.Barrier(2)
        def run(args):
            directory, version = args
            Publisher(self.store, version, SOURCE).release(directory, "https://releases.example")
        with ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(run, ((a, "1.2.3"), (b, "2.0.0"))))
        self.assertEqual(self.assert_manifest()["version"], "2.0.0")

    def test_manifest_remote_mismatch_or_missing_object_cannot_be_advertised(self):
        files = self.files("new")
        p = self.publish(files)
        before = self.store.get("app/dmg/latest.json")
        (files / "Herdr-macos-x86_64.dmg").write_bytes(b"not the committed image")
        with self.assertRaisesRegex(RuntimeError, "remote DMG checksum"):
            p.release(files, "https://releases.example")
        self.assertEqual(self.store.get("app/dmg/latest.json"), before)
        del self.store.data["app/1.2.3/Herdr.dmg"]
        with self.assertRaises(RuntimeError):
            p.release(files, "https://releases.example")

    def test_published_checksum_pins_a_repair_after_a_versioned_image_is_missing(self):
        original = self.files("original")
        p = self.publish(original)
        key = "app/1.2.3/Herdr-macos-x86_64.dmg"
        committed = self.store.data.pop(key)
        rebuilt = self.files("rebuilt", extra="new signing timestamp")
        before = dict(self.store.data)
        with self.assertRaisesRegex(RuntimeError, "published checksum"):
            p.image("macos-x86_64", rebuilt / "Herdr-macos-x86_64.dmg")
        self.assertEqual(self.store.data, before)
        p.image("macos-x86_64", original / "Herdr-macos-x86_64.dmg")
        self.assertEqual(self.store.get(key).body, committed.body)
        self.assert_manifest()

    def test_manifest_retry_after_pointer_failure_is_idempotent(self):
        files = self.files("new")
        p = Publisher(self.store, "1.2.3", SOURCE)
        for target in ("macos-aarch64", "macos-x86_64"):
            p.image(target, files / f"Herdr-{target}.dmg")
        self.store.fail.add("app/dmg/latest.json")
        with self.assertRaisesRegex(RuntimeError, "injected"):
            p.release(files, "https://releases.example")
        committed = self.store.get("app/1.2.3/dmg.json")
        self.store.fail.clear()
        p.release(files, "https://releases.example")
        self.assertEqual(self.store.get("app/1.2.3/dmg.json"), committed)
        self.assert_manifest()

    def test_conflicting_concurrent_manifest_definitions_cannot_replace_the_winner(self):
        files = self.files("new")
        p = Publisher(self.store, "1.2.3", SOURCE)
        for target in ("macos-aarch64", "macos-x86_64"):
            p.image(target, files / f"Herdr-{target}.dmg")
        self.store.barrier_key = "app/1.2.3/dmg.json"
        self.store.barrier = threading.Barrier(2)
        def run(base):
            try:
                Publisher(self.store, "1.2.3", SOURCE).release(files, base)
                return "success"
            except RuntimeError:
                return "conflict"
        with ThreadPoolExecutor(max_workers=2) as pool:
            result = list(pool.map(run, ("https://one.example", "https://two.example")))
        self.assertCountEqual(result, ["success", "conflict"])
        self.assertEqual(self.store.get("app/1.2.3/dmg.json").body, self.store.get("app/dmg/latest.json").body)
        self.assert_manifest()

    def test_semver_precedence_and_equal_precedence_build_conflict(self):
        versions = ["1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-beta", "1.0.0-rc.1", "1.0.0", "1.0.1", "2.0.0"]
        self.assertEqual(sorted(reversed(versions), key=version_key), versions)
        for bad in ("01.0.0", "1.0.0-alpha.01", "1.0.0-a..b", "1.0.0+bad..build"):
            with self.assertRaises(ValueError):
                version_key(bad)
        self.publish(self.files("a", "1.0.0+one"), "1.0.0+one")
        with self.assertRaisesRegex(RuntimeError, "same-version latest"):
            self.publish(self.files("b", "1.0.0+two"), "1.0.0+two")


class AdapterTests(unittest.TestCase):
    def test_all_put_requests_have_server_side_preconditions(self):
        calls = []
        def run(args, **kwargs):
            calls.append(args)
            return subprocess.CompletedProcess(args, 0, '{}', '')
        with mock.patch("publish_dmg.subprocess.run", side_effect=run):
            s = S3("https://r2.example", "fixture")
            s.put("app/1.2.3/test.dmg", b"image", {}, "application/x-apple-diskimage")
            s.put("app/test.dmg", b"image", {}, "application/x-apple-diskimage", '"opaque-etag"')
        self.assertEqual(calls[0][-2:], ["--if-none-match", "*"])
        self.assertEqual(calls[1][-2:], ["--if-match", '"opaque-etag"'])
        self.assertIn("public, max-age=31536000, immutable", calls[0])
        self.assertIn("no-cache", calls[1])

    def test_access_errors_are_not_missing_objects_or_write_fallbacks(self):
        with mock.patch("publish_dmg.subprocess.run", return_value=subprocess.CompletedProcess([], 254, '', 'An error occurred (AccessDenied) when calling the GetObject operation')) as run:
            with self.assertRaisesRegex(RuntimeError, "AccessDenied"):
                S3("https://r2.example", "fixture").get("app/test.dmg")
            self.assertEqual(run.call_count, 1)

class WorkflowCliTests(unittest.TestCase):
    def test_actual_workflow_retries_through_the_real_cli_and_s3_adapter(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        jobs = re.search(r"^  mac-dmg:\n(.*?)(?=^  publish:\n)", workflow, re.M | re.S).group(1)
        self.assertNotIn("aws s3 cp", jobs)
        self.assertIn("steps.dmg.outcome == 'success'", jobs)

        def block(step):
            part = jobs.split("- name: " + step + "\n", 1)[1]
            lines = re.search(r"        run: \|\n((?:          .*\n|\n)+)", part).group(1).splitlines()
            return "\n".join(line[10:] if line.startswith("          ") else line for line in lines)

        with tempfile.TemporaryDirectory(prefix="herdr-dmg-cli-") as tmp:
            work = Path(tmp)
            (work / "scripts").mkdir()
            for name in ("dmg_manifest.py", "publish_dmg.py"):
                shutil.copy2(ROOT / "scripts" / name, work / "scripts" / name)
            (work / "bin").mkdir()
            (work / "dist").mkdir()
            aws = work / "bin/aws"
            aws.write_text("#!" + sys.executable + "\n" + r'''
import hashlib,json,os,pathlib,sqlite3,sys
a=sys.argv[1:];op=a[a.index('s3api')+1]
def opt(name):return a[a.index(name)+1] if name in a else None
def fail(code):
 print(f'An error occurred ({code}) when calling the {op} operation',file=sys.stderr);sys.exit(254)
c=sqlite3.connect(os.environ['TEST_BUCKET'],isolation_level=None)
c.execute('CREATE TABLE IF NOT EXISTS objects (key TEXT PRIMARY KEY, body BLOB, meta TEXT, etag TEXT)')
key=opt('--key')
if op=='get-object':
 row=c.execute('SELECT body,meta,etag FROM objects WHERE key=?',(key,)).fetchone()
 if row is None:fail('NoSuchKey')
 pathlib.Path(a[-1]).write_bytes(row[0]);print(json.dumps({'Metadata':json.loads(row[1]),'ETag':row[2]}))
elif op=='list-objects-v2':
 print(json.dumps({'Contents':[{'Key':r[0]} for r in c.execute('SELECT key FROM objects WHERE key LIKE ?',(opt('--prefix')+'%',))]}))
elif op=='put-object':
 assert ('--if-match' in a) != ('--if-none-match' in a),'unconditional or ambiguous write'
 if key==os.environ.get('FAIL_KEY'):fail('ServiceUnavailable')
 body=pathlib.Path(opt('--body')).read_bytes();etag='"'+hashlib.md5(body).hexdigest()+'"'
 c.execute('BEGIN IMMEDIATE');old=c.execute('SELECT etag FROM objects WHERE key=?',(key,)).fetchone()
 if (opt('--if-none-match') and old) or (opt('--if-match') and (not old or old[0]!=opt('--if-match'))):
  c.execute('ROLLBACK');fail('PreconditionFailed')
 c.execute('INSERT OR REPLACE INTO objects VALUES (?,?,?,?)',(key,body,opt('--metadata'),etag));c.execute('COMMIT')
 print(json.dumps({'ETag':etag}))
else:fail('NotImplemented')
''')
            aws.chmod(0o755)
            database = work / "bucket.sqlite"
            with sqlite3.connect(database) as db:
                db.execute('CREATE TABLE objects (key TEXT PRIMARY KEY, body BLOB, meta TEXT, etag TEXT)')
                for key in ("app/1.2.2/Herdr.dmg", "app/Herdr.dmg"):
                    db.execute('INSERT INTO objects VALUES (?,?,?,?)', (key, b"previous ARM", '{}', '"legacy-etag"'))
            env = {"PATH": f"{work / 'bin'}:{Path(sys.executable).parent}:/usr/bin:/bin", "HOME": str(work),
                   "TEST_BUCKET": str(database), "PYTHONDONTWRITEBYTECODE": "1", "AWS_EC2_METADATA_DISABLED": "true",
                   "ENDPOINT": "https://r2.example", "BUCKET": "fixture", "GITHUB_REF_NAME": "v1.2.3",
                   "GITHUB_SHA": SOURCE, "RELEASES_URL": "https://releases.example"}
            def run(body, version="1.2.3", **flags):
                return subprocess.run(["/bin/bash", "-eu", "-c", body], cwd=work,
                                      env=dict(env, GITHUB_REF_NAME="v"+version, **flags), capture_output=True, text=True)
            def image(target, version="1.2.3", **flags):
                return run(block("Upload website download").replace("${{ matrix.target }}", target), version, **flags)
            def snapshot():
                with sqlite3.connect(database) as db:
                    return db.execute('SELECT key,body,meta,etag FROM objects ORDER BY key').fetchall()
            def rows():return {row[0]:row[1] for row in snapshot()}
            for target in ("macos-aarch64", "macos-x86_64"):
                (work / f"dist/Herdr-{target}.dmg").write_bytes(f"signed 1.2.3 {target}".encode())
                r = image(target);self.assertEqual(r.returncode, 0, r.stderr)
            r = run(block("Publish complete DMG release last"));self.assertEqual(r.returncode, 0, r.stderr)
            published = snapshot()
            for target in ("macos-aarch64", "macos-x86_64"):
                (work / f"dist/Herdr-{target}.dmg").write_bytes(b"regenerated signing timestamp")
                # A repeated release must not write this alias at all.
                r = image(target, FAIL_KEY="app/Herdr-macos-aarch64.dmg")
                self.assertEqual(r.returncode, 0, r.stderr)
            r = run(block("Publish complete DMG release last"));self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual(snapshot(), published)
            for target in ("macos-aarch64", "macos-x86_64"):
                (work / f"dist/Herdr-{target}.dmg").write_bytes(f"signed 1.2.4 {target}".encode())
            r = image("macos-aarch64", "1.2.4", FAIL_KEY="app/Herdr-macos-aarch64.dmg")
            self.assertNotEqual(r.returncode, 0)
            canonical = rows()["app/1.2.4/Herdr-macos-aarch64.dmg"]
            (work / "dist/Herdr-macos-aarch64.dmg").write_bytes(b"rebuilt after partial upload")
            for target in ("macos-aarch64", "macos-x86_64"):
                r = image(target, "1.2.4");self.assertEqual(r.returncode, 0, r.stderr)
            r = run(block("Publish complete DMG release last"), "1.2.4")
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual((work / "dist/Herdr-macos-aarch64.dmg").read_bytes(), canonical)
            objects = rows();doc = json.loads(objects["app/dmg/latest.json"])
            self.assertEqual(doc["version"], "1.2.4")
            for target, value in doc["targets"].items():
                self.assertEqual(hashlib.sha256(objects[f"app/1.2.4/Herdr-{target}.dmg"]).hexdigest(), value["sha256"])
            for key, body, *_ in published:
                if key.startswith("app/1.2.3/"):
                    self.assertEqual(objects[key], body)


if __name__ == "__main__":
    unittest.main()
