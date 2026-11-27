"""Offline DMG/release regressions. Native cases mount only their own images."""
import hashlib
import importlib.util
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("dmg_manifest", ROOT / "scripts/dmg_manifest.py")
metadata = importlib.util.module_from_spec(spec)
spec.loader.exec_module(metadata)


class ManifestTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="herdr-dmg-manifest-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.files = {}
        for arch in ("aarch64", "x86_64"):
            f = self.work / f"Herdr-macos-{arch}.dmg"
            f.write_bytes(f"signed and stapled {arch} fixture".encode())
            self.files[arch] = f

    def test_distinct_architecture_urls_and_actual_final_bytes(self):
        d = metadata.manifest("1.2.3", self.work, "https://releases.example/")
        for arch, f in self.files.items():
            self.assertEqual(d["targets"][f"macos-{arch}"], {
                "url": f"https://releases.example/app/1.2.3/{f.name}",
                "sha256": hashlib.sha256(f.read_bytes()).hexdigest()})
        self.files["x86_64"].write_bytes(b"later signing/stapling changes bytes")
        newer = metadata.manifest("1.2.4", self.work, "https://releases.example")
        self.assertNotEqual(d["targets"]["macos-x86_64"], newer["targets"]["macos-x86_64"])
        self.assertEqual(d["targets"]["macos-aarch64"]["sha256"], newer["targets"]["macos-aarch64"]["sha256"])

    def test_missing_empty_or_symlinked_architecture_rejects_complete_release(self):
        f = self.files["x86_64"]
        for mode in ("missing", "empty", "symlink"):
            with self.subTest(mode=mode):
                f.unlink(missing_ok=True)
                if mode == "empty":
                    f.touch()
                if mode == "symlink":
                    f.symlink_to(self.files["aarch64"])
                with self.assertRaises(ValueError):
                    metadata.manifest("1.2.3", self.work, "https://releases.example")

    def test_unsafe_release_inputs_rejected(self):
        for version in ("../1.2.3", "1.2", "1.2.3?bad", "v1.2.3"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                metadata.manifest(version, self.work, "https://releases.example")
        for base in ("http://releases.example", "https://user:pass@releases.example", "https://releases.example?q=x", "https://releases.example/#x"):
            with self.subTest(base=base), self.assertRaises(ValueError):
                metadata.manifest("1.2.3", self.work, base)


class DmgTests(unittest.TestCase):
    native = False

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="herdr-dmg-test-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.bin = self.work / "bin"
        self.bin.mkdir()
        self.repo = self.work / "repo"
        (self.repo / "scripts").mkdir(parents=True)
        shutil.copy2(ROOT / "scripts/mac-dmg", self.repo / "scripts/mac-dmg")
        self.app = self.work / "Herdr.app"
        (self.app / "Contents/MacOS").mkdir(parents=True)
        (self.app / "Contents/MacOS/Herdr").write_bytes(b"mock executable")
        (self.app / "Contents/Info.plist").write_bytes(plistlib.dumps({
            "CFBundleExecutable": "Herdr", "CFBundleIdentifier": "dev.herdr.dmg-fixture",
            "CFBundleName": "Herdr", "CFBundleVersion": "1.0.0", "LSMinimumSystemVersion": "13.0"}))
        self.log = self.work / "actions.jsonl"
        self.stub("uname", "echo Darwin")
        self.stub("lipo", 'if [ "$TEST_NATIVE" = 1 ]; then exec /usr/bin/lipo "$@"; fi; echo "$TEST_ARCH"')
        self.stub("codesign", '''if [ "$1" = -dvv ]; then echo 'Authority=Developer ID Application: TEST FIXTURE ONLY' >&2; exit 0; fi
if [ "$TEST_NATIVE" = 1 ]; then exec /usr/bin/codesign "$@"; fi
exit 0''')
        self.stub("tiffutil", "exit 0")
        self.stub("ditto", f'''if [ "$TEST_NATIVE" = 1 ]; then exec /usr/bin/ditto "$@"; fi
exec "{sys.executable}" -c 'import shutil,sys; shutil.copytree(sys.argv[1],sys.argv[2],symlinks=True)' "$@"''')
        self.stub("create-dmg", f'''exec "{sys.executable}" "{self.work / 'image.py'}" "$@"''')
        (self.work / "image.py").write_text('''import json,os,pathlib,shutil,subprocess,sys
out,source=map(pathlib.Path,sys.argv[-2:])
link=source/'Applications'
if os.environ.get('TEST_BAD_LINK')!='1':link.symlink_to('/Applications')
if os.environ.get('TEST_TAMPER')=='1':(source/'Herdr.app/Contents/MacOS/Herdr').write_bytes(b'wrong app')
if os.environ['TEST_NATIVE']=='1':
 subprocess.run(['/usr/bin/hdiutil','create','-quiet','-volname','Herdr','-srcfolder',str(source),'-format','UDZO',str(out)],check=True)
else:
 image=pathlib.Path(os.environ['TEST_IMAGE'])
 if image.exists():shutil.rmtree(image)
 shutil.copytree(source,image,symlinks=True);out.write_bytes(b'mock image')
''')
        if not self.native:
            self.stub("hdiutil", f'''exec "{sys.executable}" "{self.work / 'mount.py'}" "$@"''')
            (self.work / "mount.py").write_text('''import json,os,pathlib,shutil,sys
if sys.argv[1]=='info':print('{"images":[]}')
if sys.argv[1]=='attach':
 p=pathlib.Path(sys.argv[sys.argv.index('-mountpoint')+1]);shutil.copytree(os.environ['TEST_IMAGE'],p,symlinks=True,dirs_exist_ok=True)
''')
            self.stub("plutil", "cat")
            self.stub("jq", '''case "$*" in *Accepted*) grep -q Accepted "$3";; esac''')
        self.stub("xcrun", '''case "$*" in
  'notarytool submit '*) [ "${TEST_NOTARY_FAIL:-0}" = 0 ] || exit 19; echo '{"status":"Accepted"}' ;;
  'notarytool log '*) echo '{}' > "$5" ;;
  'stapler validate '*) [ "${TEST_STAPLE_FAIL:-0}" = 0 ] || exit 20 ;;
esac''')
        self.stub("spctl", '[ "${TEST_GATEKEEPER_FAIL:-0}" = 0 ]')
        self.stub("mac-sign", 'echo sign >> "$TEST_SIGN_LOG"', self.repo / "scripts/mac-sign")

    def stub(self, name, body, path=None):
        path = path or self.bin / name
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(0o755)

    def run_dmg(self, arch="x86_64", target="macos-x86_64", output_name="output.dmg", **flags):
        out = self.work / output_name
        out.unlink(missing_ok=True)
        env = {"PATH": f"{self.bin}:/usr/bin:/bin:/usr/local/bin", "HOME": str(self.work / "home"),
               "TMPDIR": str(self.work), "TEST_IMAGE": str(self.work / "image"), "TEST_ARCH": arch,
               "TEST_NATIVE": str(int(self.native)), "TEST_SIGN_LOG": str(self.work / "sign.log"),
               "APPLE_CERTIFICATE_P12": "TEST-ONLY", "APPLE_CERTIFICATE_PASSWORD": "TEST-ONLY",
               "APPLE_API_KEY": "TEST-ONLY", "APPLE_API_KEY_ID": "TEST-ONLY", "APPLE_API_ISSUER": "TEST-ONLY", **flags}
        r = subprocess.run([str(self.repo / "scripts/mac-dmg"), str(self.app), str(out), target],
                           env=env, cwd=self.repo, text=True, capture_output=True)
        return r, out

    def test_each_thin_architecture_preserves_payload_and_applications_link(self):
        for arch, target in (("x86_64", "macos-x86_64"), ("arm64", "macos-aarch64")):
            with self.subTest(arch=arch):
                r, out = self.run_dmg(arch, target)
                self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
                self.assertTrue(out.is_file())

    def test_wrong_universal_or_unknown_architecture_stops_before_signing(self):
        for arch, target in (("arm64", "macos-x86_64"), ("x86_64", "macos-aarch64"), ("x86_64 arm64", "macos-x86_64"), ("i386", "macos-x86_64")):
            with self.subTest(arch=arch, target=target):
                r, out = self.run_dmg(arch, target)
                self.assertNotEqual(r.returncode, 0)
                self.assertFalse(out.exists())
                self.assertFalse((self.work / "sign.log").exists())

    def test_bad_image_contents_or_notarization_never_publish_output(self):
        for flag in ("TEST_BAD_LINK", "TEST_TAMPER", "TEST_NOTARY_FAIL", "TEST_STAPLE_FAIL", "TEST_GATEKEEPER_FAIL"):
            with self.subTest(flag=flag):
                r, out = self.run_dmg(**{flag: "1"})
                self.assertNotEqual(r.returncode, 0, r.stdout + r.stderr)
                self.assertFalse(out.exists())

    def test_named_output_cannot_mislabel_a_manual_build_without_target_argument(self):
        for arch, name in (("arm64", "Herdr-macos-x86_64.dmg"), ("x86_64", "Herdr-macos-aarch64.dmg"), ("x86_64", "Herdr-macos-unknown.dmg")):
            with self.subTest(arch=arch, name=name):
                r, out = self.run_dmg(arch, "", output_name=name)
                self.assertNotEqual(r.returncode, 0)
                self.assertIn("DMG filename does not match", r.stderr)
                self.assertFalse(out.exists())
                self.assertFalse((self.work / "sign.log").exists())

    def test_portable_fixture_has_no_dependency_on_host_apple_tools(self):
        posix = self.work / "posix-tools"
        posix.mkdir()
        for name in ("basename", "dirname", "date", "mktemp", "mkdir", "tee", "rm", "mv", "cat", "readlink", "cmp", "grep"):
            (posix / name).symlink_to(shutil.which(name))
        original = subprocess.run

        def run(*args, **kwargs):
            env = kwargs.get("env")
            if env and env.get("TEST_NATIVE") == "0":
                kwargs["env"] = dict(env, PATH=f"{self.bin}:{posix}")
            return original(*args, **kwargs)

        with mock.patch.object(subprocess, "run", side_effect=run):
            for arch, target in (("x86_64", "macos-x86_64"), ("arm64", "macos-aarch64")):
                result, out = self.run_dmg(arch, target)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue(out.is_file())


@unittest.skipUnless(sys.platform == "darwin" and all(shutil.which(t) for t in ("hdiutil", "jq", "xcrun", "lipo", "codesign")), "native macOS image tools required")
class NativeDmgTests(unittest.TestCase):
    native = True
    setUp = DmgTests.setUp
    stub = DmgTests.stub
    run_dmg = DmgTests.run_dmg

    # The inherited mock-architecture cases are portable tests. This test makes
    # actual thin binaries and real compressed images, with simulated notary
    # responses and an ad-hoc app signature (no credentials/keychain access).
    def test_each_thin_architecture_preserves_payload_and_applications_link(self):
        source = self.work / "main.c"
        source.write_text("int main(void) { return 0; }\n")
        for arch, target in (("x86_64", "macos-x86_64"), ("arm64", "macos-aarch64")):
            with self.subTest(arch=arch):
                subprocess.run(["xcrun", "clang", "-target", f"{arch}-apple-macos13", str(source), "-o", str(self.app / "Contents/MacOS/Herdr")], check=True, capture_output=True)
                subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", str(self.app)], check=True, capture_output=True)
                r, out = self.run_dmg(arch, target)
                self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
                self.assertTrue(out.is_file())

if __name__ == "__main__":
    unittest.main()
