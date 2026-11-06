import json
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("fork-sync.py")


def git(where, *args):
    return subprocess.check_output(["git", "-C", str(where), *args], text=True, stderr=subprocess.DEVNULL).strip()


class ForkSync(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="herdr-fork-sync-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.upstream = self.root / "upstream"
        self.upstream.mkdir()
        git(self.upstream, "init", "-b", "master")
        self.identity(self.upstream)
        self.commit(self.upstream, "base.txt", "base")
        self.fork = self.root / "fork.git"
        git(self.root, "clone", "--bare", str(self.upstream), str(self.fork))
        self.work = self.root / "work"
        git(self.root, "clone", str(self.fork), str(self.work))
        self.identity(self.work)
        self.commit(self.work, "fork.txt", "browser support")
        git(self.work, "push", "origin", "master")
        self.before = git(self.fork, "rev-parse", "master")

    def identity(self, repo):
        git(repo, "config", "user.email", "test@example.invalid")
        git(repo, "config", "user.name", "Test")

    def commit(self, repo, file, value):
        (repo / file).write_text(value)
        git(repo, "add", file)
        git(repo, "commit", "-m", "fixture")

    def sync(self, *args, ok=True):
        result = subprocess.run(["python3", str(SCRIPT), *args], cwd=self.work, text=True, capture_output=True)
        if ok:
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0)

    def prepare(self, ok=True):
        return self.sync("prepare", "--upstream", str(self.upstream), "--branch", "automation/upstream-test", ok=ok)

    def test_verified_candidate_keeps_fork_changes_and_lands_by_fast_forward(self):
        self.commit(self.upstream, "upstream.txt", "new upstream")
        candidate = self.prepare()
        self.assertTrue(candidate["changed"])
        self.assertEqual(git(self.work, "show", candidate["sha"] + ":fork.txt"), "browser support")
        self.assertEqual(git(self.fork, "rev-parse", "master"), self.before)
        landed = self.sync("land", "--branch", candidate["branch"], "--sha", candidate["sha"])
        self.assertTrue(landed["landed"])
        self.assertEqual(git(self.fork, "rev-parse", "master"), candidate["sha"])
        self.assertEqual(git(self.fork, "show", "master:upstream.txt"), "new upstream")

    def test_no_upstream_change_creates_no_candidate(self):
        result = self.prepare()
        self.assertFalse(result["changed"])
        self.assertEqual(git(self.fork, "rev-parse", "master"), self.before)
        self.assertEqual(git(self.fork, "for-each-ref", "--format=%(refname)", "refs/heads/automation/"), "")

    def test_conflict_does_not_change_remote_master(self):
        self.commit(self.work, "base.txt", "fork version")
        git(self.work, "push", "origin", "master")
        before = git(self.fork, "rev-parse", "master")
        self.commit(self.upstream, "base.txt", "upstream version")
        self.prepare(ok=False)
        self.assertEqual(git(self.fork, "rev-parse", "master"), before)

    def test_a_concurrent_master_change_is_not_overwritten(self):
        self.commit(self.upstream, "upstream.txt", "new upstream")
        candidate = self.prepare()
        git(self.work, "switch", "master")
        self.commit(self.work, "later.txt", "another task")
        git(self.work, "push", "origin", "master")
        before = git(self.fork, "rev-parse", "master")
        self.sync("land", "--branch", candidate["branch"], "--sha", candidate["sha"], ok=False)
        self.assertEqual(git(self.fork, "rev-parse", "master"), before)


if __name__ == "__main__":
    unittest.main()
