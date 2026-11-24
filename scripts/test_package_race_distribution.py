"""Two real Cargo builds in different worktrees, with a controlled copy race."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parent.parent
TOOLS = all(shutil.which(tool) for tool in ("cargo", "rustc", "git", "bash"))


def dependency_worktrees(work, script):
    """Same versions, different path dependency contents and shared build-dir."""
    work = Path(work)
    for name in ("home", "tmp", "tools", "hooks"):
        (work / name).mkdir()
    env = {"HOME":str(work / "home"), "TMPDIR":str(work / "tmp"), "PATH":os.environ["PATH"],
           "CARGO_HOME":str(work / "home/cargo"), "CARGO_NET_OFFLINE":"true",
           "CARGO_TARGET_DIR":str(work / "shared-target"), "GIT_CONFIG_GLOBAL":"/dev/null",
           "GIT_CONFIG_NOSYSTEM":"1", "LC_ALL":"C"}
    for key in ("RUSTC", "RUSTUP_HOME"):
        if key in os.environ:
            env[key] = os.environ[key]
    a, b = work / "source-a", work / "source-b"
    (a / "src").mkdir(parents=True)
    (a / "origin/src").mkdir(parents=True)
    (a / "Cargo.toml").write_text('[workspace]\n[package]\nname="herdr-app"\nversion = "0.1.17"\nedition="2024"\n[dependencies]\norigin={path="origin"}\n')
    (a / "origin/Cargo.toml").write_text('[package]\nname="origin"\nversion="0.1.0"\nedition="2024"\n')
    (a / "origin/src/lib.rs").write_text('pub const ORIGIN: &str = "source-a";\n')
    (a / "src/main.rs").write_text('fn main(){if std::env::args().any(|a|a=="--source"){println!("{}",origin::ORIGIN)}else{println!("herdr-app {}",env!("CARGO_PKG_VERSION"))}}\n')
    for name in (".cargo/config.toml", "assets/icons/Herdr.icns", "assets/icons/herdr-ui-icon-clean.svg", "LICENSE", "THIRD-PARTY-NOTICES", "scripts/mac-sign"):
        dest = a / name; dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, dest)
    shutil.copy2(script, a / "scripts/package")
    if (ROOT / "scripts/package_output.py").exists():
        shutil.copy2(ROOT / "scripts/package_output.py", a / "scripts/package_output.py")
    with (a / ".cargo/config.toml").open("a") as file:
        file.write('\n[build]\nbuild-dir=' + json.dumps(str(work / "shared-build")) + '\n')
    (a / ".gitignore").write_text("dist/\ntarget/\n")
    def run(argv, cwd=a):
        return subprocess.run(argv, cwd=cwd, env=env, text=True, capture_output=True, check=True)
    run(["cargo", "generate-lockfile", "--offline"])
    run(["git", "init", "-q"]); run(["git", "add", "."])
    commit = ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false", "-c", f"core.hooksPath={work / 'hooks'}", "commit", "-qm"]
    run([*commit, "dependency A"])
    a_sha = run(["git", "rev-parse", "HEAD"]).stdout.strip()
    run(["git", "worktree", "add", "--detach", str(b), "HEAD"])
    (b / "origin/src/lib.rs").write_text('pub const ORIGIN: &str = "source-b";\n')
    run(["git", "add", "origin/src/lib.rs"], b); run([*commit, "same version, dependency B"], b)
    b_sha = run(["git", "rev-parse", "HEAD"], b).stdout.strip()
    native = run(["rustc", "-vV"]).stdout.split("host: ", 1)[1].splitlines()[0]
    return env, a, b, native, {"source_a_commit":a_sha, "source_b_commit":b_sha}


def archived_source(folder, env, work):
    archive, = (folder / "dist").glob("*.tar.gz")
    member = "Herdr.app/Contents/MacOS/Herdr" if sys.platform == "darwin" else "herdr-app/herdr-app"
    with tarfile.open(archive) as tar:
        payload = tar.extractfile(member).read()
    binary = work / "inspect-executable"
    binary.write_bytes(payload); binary.chmod(0o755)
    source = subprocess.check_output([str(binary), "--source"], env=env, text=True).strip()
    receipt = json.loads(archive.with_suffix("").with_suffix(".build.json").read_text())
    return {"source":source, "payload_sha256":hashlib.sha256(payload).hexdigest(),
            "archive_sha256":hashlib.sha256(archive.read_bytes()).hexdigest(), "receipt":receipt}


def race(work, script, *, independent_other=False):
    """Keep the tiny fixture and logs when called by an external evidence probe."""
    work = Path(work)
    for name in ("home", "tmp", "tools", "hooks"):
        (work / name).mkdir()
    env = dict(os.environ, HOME=str(work / "home"), TMPDIR=str(work / "tmp"),
               CARGO_HOME=str(work / "home/cargo"), CARGO_TARGET_DIR=str(work / "shared"),
               CARGO_NET_OFFLINE="true", SYNC=str(work))
    a, b = work / "source-a", work / "source-b"
    (a / "src").mkdir(parents=True)
    (a / "Cargo.toml").write_text('[workspace]\n[package]\nname="herdr-app"\nversion = "0.1.17"\nedition="2024"\n')
    def source(label):
        return 'fn main() { if std::env::args().any(|a| a == "--source") { println!("' + label + '"); } else { println!("herdr-app {}", env!("CARGO_PKG_VERSION")); } }\n'
    (a / "src/main.rs").write_text(source("source-a"))
    (a / ".gitignore").write_text("target/\ndist/\nsign-source\n")
    for name in (".cargo/config.toml", "assets/icons/Herdr.icns", "assets/icons/herdr-ui-icon-clean.svg", "LICENSE", "THIRD-PARTY-NOTICES"):
        dst = a / name
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, dst)
    (a / "scripts").mkdir()
    shutil.copy2(script, a / "scripts/package")
    if (ROOT / "scripts/package_output.py").exists():
        shutil.copy2(ROOT / "scripts/package_output.py", a / "scripts/package_output.py")
    sign = a / "scripts/mac-sign"
    sign.write_text('#!/bin/sh\n"$1/Contents/MacOS/Herdr" --source > sign-source\n')
    sign.chmod(0o755)
    def run(argv, cwd=a):
        return subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True, check=True)
    run(["cargo", "generate-lockfile", "--offline"])
    run(["git", "init", "-q"])
    run(["git", "add", "."])
    commit = ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false", "-c", f"core.hooksPath={work / 'hooks'}", "commit", "-qm"]
    run([*commit, "source a"])
    a_sha = run(["git", "rev-parse", "HEAD"]).stdout.strip()
    run(["git", "worktree", "add", "--detach", str(b), "HEAD"])
    (b / "src/main.rs").write_text(source("source-b"))
    run(["git", "add", "src/main.rs"], b)
    run([*commit, "source b, same package version"], b)
    b_sha = run(["git", "rev-parse", "HEAD"], b).stdout.strip()
    assert a_sha != b_sha
    barrier = work / "tools/tar"
    barrier.write_text('''#!/bin/sh
if [ "$1" = '-czf' ]; then
  touch "$SYNC/verified"
  attempts=0
  while [ ! -f "$SYNC/other-built" ]; do
    attempts=$((attempts+1)); [ "$attempts" -lt 600 ] || exit 97
    sleep .05
  done
fi
exec /usr/bin/tar "$@"
''')
    barrier.chmod(0o755)
    native = run(["rustc", "-vV"]).stdout.split("host: ", 1)[1].splitlines()[0]
    command = ["bash", "scripts/package", native]
    with (work / "package.stdout").open("w") as stdout, (work / "package.stderr").open("w") as stderr:
        child = subprocess.Popen(command, cwd=a, env={**env, "PATH":str(work / "tools") + os.pathsep + env["PATH"]}, stdout=stdout, stderr=stderr)
        try:
            deadline = time.monotonic() + 40
            while not (work / "verified").exists():
                if child.poll() is not None:
                    raise AssertionError((work / "package.stderr").read_text())
                if time.monotonic() > deadline:
                    raise AssertionError("package never reached its post-inspection boundary")
                time.sleep(.05)
            build_env = {**env}
            if independent_other:
                build_env["CARGO_TARGET_DIR"] = str(work / "other-target")
            # Both worktrees have the same name/version. Refresh B's mtime
            # after A compiled, so Cargo must really compile B rather than
            # treating A's newer shared fingerprint as a cache hit.
            (b / "src/main.rs").touch()
            other = subprocess.run(["cargo", "build", "--offline", "--locked", "--release", "--target", native], cwd=b, env=build_env, text=True, capture_output=True)
            (work / "other-build.stdout").write_text(other.stdout)
            (work / "other-build.stderr").write_text(other.stderr)
            assert other.returncode == 0, other.stderr
            other_binary = Path(build_env["CARGO_TARGET_DIR"]) / native / "release/herdr-app"
            other_source = run([str(other_binary), "--source"], b).stdout.strip()
            assert other_source == "source-b", (other_source, other.stderr)
        finally:
            (work / "other-built").touch()
            try:
                code = child.wait(timeout=20)
            except subprocess.TimeoutExpired:
                child.kill(); child.wait()
                raise
    assert code == 0, (work / "package.stderr").read_text()
    archive, = (a / "dist").glob("*.tar.gz")
    unpack = work / "unpacked"
    unpack.mkdir()
    with tarfile.open(archive) as tar:
        tar.extractall(unpack)
    executable = unpack / ("Herdr.app/Contents/MacOS/Herdr" if sys.platform == "darwin" else "herdr-app/herdr-app")
    version = run([str(executable), "--version"]).stdout.strip()
    actual_source = run([str(executable), "--source"]).stdout.strip()
    row = {"command":command,"source_a_commit":a_sha,"source_b_commit":b_sha,"both_manifest_versions":"0.1.17","other_build_uses_independent_target":independent_other,"other_compiled_source":other_source,"packed_version":version,"packed_source":actual_source,"archive_sha256":hashlib.sha256(archive.read_bytes()).hexdigest(),"package_script_sha256":hashlib.sha256(Path(script).read_bytes()).hexdigest(),"work":str(work)}
    (work / "result.json").write_text(json.dumps(row, indent=2) + "\n")
    return row


@unittest.skipUnless(TOOLS and sys.platform in ("darwin", "linux"), "requires native POSIX Rust and Git tools")
class CargoRaceTests(unittest.TestCase):
    def test_same_version_other_worktree_cannot_replace_the_packaged_source(self):
        with tempfile.TemporaryDirectory(prefix="herdr-real-cargo-race-") as tmp:
            result = race(tmp, ROOT / "scripts/package")
            self.assertEqual(result["packed_version"], "herdr-app 0.1.17")
            self.assertEqual(result["packed_source"], "source-a")

    def test_shared_build_dir_cannot_reuse_another_worktrees_local_dependency(self):
        with tempfile.TemporaryDirectory(prefix="herdr-local-dependency-") as tmp:
            work = Path(tmp)
            env, a, b, native, _ = dependency_worktrees(work, ROOT / "scripts/package")
            builds = []
            for folder, expected in ((b, "source-b"), (a, "source-a"), (a, "source-a")):
                if folder == a and not builds[1:]:
                    (a / "src/main.rs").touch()
                q = subprocess.run(["bash", "scripts/package", native], cwd=folder, env=env, text=True, capture_output=True)
                self.assertEqual(q.returncode, 0, q.stderr)
                result = archived_source(folder, env, work)
                self.assertEqual(result["source"], expected)
                self.assertNotEqual(Path(result["receipt"]["private_build_dir"]), work / "shared-build")
                builds.append(result)
            self.assertEqual(len({r["receipt"]["private_build_dir"] for r in builds}), 3)

    def test_same_checkout_parallel_target_is_exclusive_and_retry_keeps_pair_consistent(self):
        with tempfile.TemporaryDirectory(prefix="herdr-dist-exclusion-") as tmp:
            work = Path(tmp)
            env, a, _, native, _ = dependency_worktrees(work, ROOT / "scripts/package")
            barrier = work / "tools/tar"
            barrier.write_text('''#!/bin/sh
if [ "$1" = -xOf ] && [ "$PACKAGER" = first ] && [ ! -f "$SYNC/go" ]; then
  printf '%s' "$2" > "$SYNC/staged-archive"
  touch "$SYNC/ready"
  attempts=0
  while [ ! -f "$SYNC/go" ]; do attempts=$((attempts+1)); [ "$attempts" -lt 600 ] || exit 97; sleep .05; done
fi
exec /usr/bin/tar "$@"
''')
            barrier.chmod(0o755)
            env.update(PATH=str(work / "tools") + os.pathsep + env["PATH"], SYNC=str(work))
            command = ["bash", "scripts/package", native]
            with (work / "first.out").open("w") as out, (work / "first.err").open("w") as err:
                child = subprocess.Popen(command, cwd=a, env={**env, "PACKAGER":"first", "CARGO_PROFILE_RELEASE_OPT_LEVEL":"0"}, stdout=out, stderr=err)
                try:
                    deadline = time.monotonic() + 40
                    while not (work / "ready").exists():
                        self.assertIsNone(child.poll(), (work / "first.err").read_text())
                        self.assertLess(time.monotonic(), deadline)
                        time.sleep(.05)
                    busy = subprocess.run(command, cwd=a, env={**env, "PACKAGER":"second", "CARGO_PROFILE_RELEASE_OPT_LEVEL":"3"}, text=True, capture_output=True)
                    self.assertNotEqual(busy.returncode, 0)
                    self.assertIn("target is locked", busy.stderr)
                finally:
                    (work / "go").touch()
                    try:
                        child.wait(timeout=20)
                    except subprocess.TimeoutExpired:
                        child.kill(); child.wait()
                        raise
            self.assertEqual(child.returncode, 0, (work / "first.err").read_text())
            first = archived_source(a, env, work)
            self.assertEqual(first["payload_sha256"], first["receipt"]["packaged_executable_sha256"])
            retry = subprocess.run(command, cwd=a, env={**env, "PACKAGER":"second", "CARGO_PROFILE_RELEASE_OPT_LEVEL":"3"}, text=True, capture_output=True)
            self.assertEqual(retry.returncode, 0, retry.stderr)
            second = archived_source(a, env, work)
            self.assertNotEqual(first["payload_sha256"], second["payload_sha256"])
            self.assertEqual(second["payload_sha256"], second["receipt"]["packaged_executable_sha256"])
            self.assertEqual(second["archive_sha256"], second["receipt"]["archive_sha256"])


if __name__ == "__main__":
    unittest.main()
