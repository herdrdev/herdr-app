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
    barrier = work / "tools/mkdir"
    barrier.write_text('''#!/bin/sh
if [ "$1 $2" = '-p dist' ]; then
  touch "$SYNC/verified"
  attempts=0
  while [ ! -f "$SYNC/other-built" ]; do
    attempts=$((attempts+1)); [ "$attempts" -lt 600 ] || exit 97
    sleep .05
  done
fi
exec /bin/mkdir "$@"
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


if __name__ == "__main__":
    unittest.main()
