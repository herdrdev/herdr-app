"""Distribution architecture guards, without downloads or release builds.

Run: uv run --no-project python -m unittest discover -s scripts -p test_distribution.py
"""

import hashlib
import os
import plistlib
import re
import shutil
import sys
import tarfile
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
MIN_MACOS = re.search(r'^MACOSX_DEPLOYMENT_TARGET = "([0-9.]+)"$',
                      (ROOT / ".cargo/config.toml").read_text(), re.M).group(1)
NATIVE_MAC = sys.platform == "darwin" and all(shutil.which(tool) for tool in ("xcrun", "lipo", "otool"))


class DistributionTests(unittest.TestCase):
    def setUp(self):
        self.work = tempfile.TemporaryDirectory(prefix="herdr-distribution-")
        self.addCleanup(self.work.cleanup)
        self.bin = Path(self.work.name)
        self.stub("uname", 'case "$1" in -s) echo "$TEST_OS";; -m) echo "$TEST_ARCH";; esac')
        self.stub("curl", 'echo "unexpected download" >&2; exit 97')
        self.stub("cargo", 'echo "unexpected build" >&2; exit 97')
        self.stub("sysctl", 'exit 1')
        self.stub("sw_vers", 'echo "${TEST_MACOS_VERSION:-15.0}"')
        self.stub("rustc", 'printf "host: %s\\n" "$TEST_RUST_HOST"')

    def stub(self, name, body):
        path = self.bin / name
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(0o755)

    def run_script(self, script, os_name, arch, *args, macos_version="15.0", rust_host=None):
        rust_arch = "aarch64" if arch in ("arm64", "aarch64") else "x86_64"
        env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}", TEST_OS=os_name, TEST_ARCH=arch,
                   TEST_MACOS_VERSION=macos_version, TEST_RUST_HOST=rust_host or f"{rust_arch}-apple-darwin",
                   RUSTC=str(self.bin / "rustc"), HOME=str(self.bin / "home"), TMPDIR=str(self.bin))
        return subprocess.run(
            [str(ROOT / script), *args], cwd=ROOT, env=env,
            capture_output=True, text=True, check=False,
        )

    def test_mac_installer_rejects_older_systems_before_download_or_replacement(self):
        installed = self.bin / "home/Applications/Herdr.app/Contents/Info.plist"
        installed.parent.mkdir(parents=True)
        installed.write_bytes(b"previous installed app")
        previous_major = int(MIN_MACOS.split(".")[0]) - 1
        for version in ("10.14.6", "10.15.7", "11.7.10", f"{previous_major}.99.99", "unknown", "13..0"):
            for arch in ("x86_64", "arm64"):
                with self.subTest(version=version, arch=arch):
                    result = self.run_script("distribution/install.sh", "Darwin", arch, macos_version=version)
                    self.assertEqual(result.returncode, 1)
                    self.assertIn("macOS", result.stderr)
                    self.assertNotIn("unexpected download", result.stderr)
                    self.assertEqual(installed.read_bytes(), b"previous installed app")

    def test_mac_installer_admits_the_dependency_floor_and_newer_versions(self):
        for version in (MIN_MACOS, MIN_MACOS + ".1", "15.8.2", "26.0"):
            for arch in ("x86_64", "arm64"):
                with self.subTest(version=version, arch=arch):
                    result = self.run_script("distribution/install.sh", "Darwin", arch, macos_version=version)
                    self.assertIn("unexpected download", result.stderr)
                    self.assertNotIn("requires macOS", result.stderr)

    def check_mac_installer_target(self, arch, target):
        self.stub("curl", '''url=; out=; prev=
for a; do
  if [ "$prev" = -o ]; then out=$a; else case $a in -*) ;; *) url=$a ;; esac; fi
  prev=$a
done
case $url in
  */latest.json)
    printf '%s' '{"version":"test","targets":{"macos-x86_64":{"url":"intel-archive","sha256":"test"},"macos-aarch64":{"url":"arm-archive","sha256":"test"}}}' > "$out" ;;
  *) echo "archive requested: $url" >&2; exit 22 ;;
esac''')
        result = self.run_script("distribution/install.sh", "Darwin", arch)
        self.assertEqual(result.returncode, 1)
        self.assertIn(f"Installing Herdr test for {target}", result.stdout)
        archive = "intel-archive" if target == "macos-x86_64" else "arm-archive"
        self.assertIn(f"archive requested: {archive}", result.stderr)
        self.assertIn("download failed", result.stderr)

    def test_intel_mac_installer_uses_x86_64(self):
        for arch in ("x86_64", "amd64"):
            with self.subTest(arch=arch):
                self.check_mac_installer_target(arch, "macos-x86_64")

    def test_intel_mac_installer_does_not_fall_back_to_an_arm_release(self):
        self.stub("curl", '''while [ "$#" -gt 0 ]; do
  if [ "$1" = -o ]; then
    printf '%s' '{"version":"test","targets":{"macos-aarch64":{"url":"arm-archive","sha256":"test"}}}' > "$2"
    exit 0
  fi
  shift
done
echo "unexpected archive download" >&2
exit 97''')
        result = self.run_script("distribution/install.sh", "Darwin", "x86_64")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no build for macos-x86_64 yet", result.stderr)
        self.assertNotIn("unexpected archive download", result.stderr)

    def test_windows_shells_are_pointed_at_the_powershell_installer(self):
        for os_name in ("MINGW64_NT-10.0-26300", "MSYS_NT-10.0-26300", "CYGWIN_NT-10.0-26300"):
            with self.subTest(os_name=os_name):
                result = self.run_script("distribution/install.sh", os_name, "x86_64")
                self.assertEqual(result.returncode, 1)
                self.assertIn("/app/install.ps1 | iex", result.stderr)
                self.assertNotIn("unexpected download", result.stderr)

    def test_unknown_os_installer_names_it(self):
        result = self.run_script("distribution/install.sh", "FreeBSD", "x86_64")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no build for FreeBSD", result.stderr)
        self.assertNotIn("unexpected download", result.stderr)

    def test_rosetta_mac_installer_uses_arm64(self):
        for key in ("hw.optional.arm64", "sysctl.proc_translated"):
            for arch in ("x86_64", "amd64"):
                with self.subTest(key=key, arch=arch):
                    self.stub("sysctl", f'[ "$2" = "{key}" ] && echo 1')
                    self.check_mac_installer_target(arch, "macos-aarch64")

    def test_native_arm_mac_installer_uses_arm64(self):
        for arch in ("arm64", "aarch64"):
            with self.subTest(arch=arch):
                self.check_mac_installer_target(arch, "macos-aarch64")

    def download_url(self, url, releases="https://releases.example", installed=False):
        """The URL install.sh downloads the app from, for a manifest giving `url`."""
        home = Path(self.work.name) / "home"
        data = home / ".local/share"
        shutil.rmtree(data / "herdr-app", ignore_errors=True)
        (data / "herdr-app" if installed else data).mkdir(parents=True, exist_ok=True)
        log = Path(self.work.name) / "urls"
        log.unlink(missing_ok=True)
        manifest = Path(self.work.name) / "latest.json"
        manifest.write_text('{"version":"1.0.0","targets":{"linux-x86_64":{"url":"%s","sha256":"x"}}}' % url)
        self.stub("curl", f'''out=; url=; prev=
for a; do
  if [ "$prev" = -o ]; then out=$a; else case $a in -*) ;; *) url=$a ;; esac; fi
  prev=$a
done
echo "$url" >> "{log}"
case $url in */latest.json) cp "{manifest}" "$out"; exit 0 ;; esac
exit 22''')
        env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}", TEST_OS="Linux", TEST_ARCH="x86_64",
                   HOME=str(home), XDG_DATA_HOME=str(data), HERDR_RELEASES_URL=releases)
        result = subprocess.run([str(ROOT / "distribution/install.sh")], cwd=ROOT, env=env,
                                capture_output=True, text=True, check=False)
        self.assertIn("download failed", result.stderr)
        return log.read_text().splitlines()[-1]

    def test_installer_marks_plain_release_downloads_for_the_counter(self):
        r = "https://releases.example"
        tarball = f"{r}/app/1.0.0/herdr-app-linux-x86_64.tar.gz"
        self.assertEqual(self.download_url(tarball), f"{tarball}?src=install")
        self.assertEqual(self.download_url(tarball, installed=True), f"{tarball}?src=reinstall")
        # A mirror's query (a signed URL) or fragment, or another host: exactly as given.
        for url in (f"{tarball}?sig=a%2Fb&exp=1", f"{tarball}?src=update", f"{tarball}#x",
                    "https://mirror.example/app/1.0.0/herdr-app-linux-x86_64.tar.gz",
                    "https://releases.example.evil/app/1.0.0/herdr-app-linux-x86_64.tar.gz"):
            with self.subTest(url=url):
                self.assertEqual(self.download_url(url), url)
        # A test server elsewhere marks its own URLs.
        local = "http://127.0.0.1:8000/e2e"
        self.assertEqual(self.download_url(f"{local}/app/1.0.0/a.tar.gz", releases=local), f"{local}/app/1.0.0/a.tar.gz?src=install")

    def mac_installer(self, signature, status, verify_status=0, arch="arm64"):
        home = self.bin / "home"
        (home / ".local/bin").mkdir(parents=True)
        (home / ".local/bin/herdr-core").touch()
        source = self.bin / "source/Herdr.app/Contents"
        source.mkdir(parents=True)
        plist = b"signed release plist must stay unchanged"
        (source / "Info.plist").write_bytes(plist)
        archive = self.bin / "release.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(source.parent, arcname="Herdr.app")
        sha = hashlib.sha256(archive.read_bytes()).hexdigest()
        manifest = self.bin / "manifest.json"
        target = "macos-x86_64" if arch == "x86_64" else "macos-aarch64"
        manifest.write_text('{"version":"1.0.0","targets":{"%s":{"url":"release","sha256":"%s"}}}' % (target, sha))
        self.stub("curl", f'''url=; out=; prev=
for a; do
  if [ "$prev" = -o ]; then out=$a; else case $a in -*) ;; *) url=$a ;; esac; fi
  prev=$a
done
case $url in */latest.json) cp "{manifest}" "$out" ;; release) cp "{archive}" "$out" ;; *) exit 97 ;; esac''')
        installed = home / "Applications/Herdr.app/Contents/Info.plist"
        installed.parent.mkdir(parents=True)
        installed.write_bytes(b"previous installed app")
        fixture = self.bin / "signature.txt"
        fixture.write_text(signature)
        log = self.bin / "codesign.log"
        self.stub("codesign", f'''echo "$*" >> "{log}"
case "$1" in
  -dvv) cat "{fixture}" >&2; exit {status} ;;
  --verify) exit {verify_status} ;;
  --force) exit 0 ;;
  *) exit 97 ;;
esac''')
        plist_log = self.bin / "plist.log"
        self.stub("PlistBuddy", f'''echo "$*" >> "{plist_log}"
printf 'dev PATH added' >> "$3"''')
        # Remap only the system tools in a scratch copy; production uses absolute paths.
        script = (ROOT / "distribution/install.sh").read_text()
        self.assertNotIn("\n    codesign ", script)
        script = script.replace("/usr/bin/codesign", str(self.bin / "codesign"))
        script = script.replace("/usr/libexec/PlistBuddy", str(self.bin / "PlistBuddy"))
        self.stub("lsregister", "exit 0")
        script = script.replace("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
                                str(self.bin / "lsregister"))
        installer = self.bin / "install.sh"
        installer.write_text(script)
        env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}", TEST_OS="Darwin",
                   TEST_ARCH=arch, HOME=str(home))
        result = subprocess.run(["sh", str(installer)], cwd=ROOT, env=env,
                                capture_output=True, text=True, check=False)
        return result, installed.read_bytes(), log.read_text().splitlines(), plist_log.exists(), plist

    def test_mac_installer_preserves_certificate_signed_bundle(self):
        fixture = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, installed, calls, edited, plist = self.mac_installer(fixture, 0)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(installed, plist)
        self.assertFalse(edited)
        self.assertEqual(len(calls), 2)
        self.assertTrue(calls[0].startswith("-dvv "))
        self.assertTrue(calls[1].startswith("--verify --deep --strict "))

    def test_intel_mac_installer_preserves_certificate_signed_bundle(self):
        fixture = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, installed, calls, edited, plist = self.mac_installer(fixture, 0, arch="x86_64")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(installed, plist)
        self.assertFalse(edited)
        self.assertEqual(len(calls), 2)
        self.assertTrue(calls[1].startswith("--verify --deep --strict "))

    def test_mac_installer_prepares_adhoc_bundle(self):
        self.check_dev_installer("adhoc", 0)

    def test_mac_installer_prepares_unsigned_bundle(self):
        self.check_dev_installer("unsigned", 1)

    def check_dev_installer(self, fixture_name, status):
        fixture = (ROOT / f"scripts/fixtures/codesign/{fixture_name}.txt").read_text()
        result, installed, calls, edited, plist = self.mac_installer(fixture, status)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(installed, plist + b"dev PATH added")
        self.assertTrue(edited)
        self.assertEqual(len(calls), 2)
        self.assertTrue(calls[1].startswith("--force --sign - "))

    def test_mac_installer_unknown_signature_leaves_installed_app_untouched(self):
        result, installed, calls, edited, _ = self.mac_installer("codesign: internal error", 1)
        self.assertEqual(result.returncode, 1)
        self.assertIn("cannot determine app signature: codesign: internal error", result.stderr)
        self.assertEqual(installed, b"previous installed app")
        self.assertFalse(edited)
        self.assertEqual(len(calls), 1)
        self.assertTrue(calls[0].startswith("-dvv "))

    def test_mac_installer_invalid_release_leaves_installed_app_untouched(self):
        fixture = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, installed, calls, edited, _ = self.mac_installer(fixture, 0, verify_status=1)
        self.assertEqual(result.returncode, 1)
        self.assertIn("invalid app signature", result.stderr)
        self.assertEqual(installed, b"previous installed app")
        self.assertFalse(edited)
        self.assertEqual(len(calls), 2)
        self.assertTrue(calls[1].startswith("--verify --deep --strict "))

    def test_intel_mac_package_builds_native_and_explicit_targets(self):
        for args in ((), ("x86_64-apple-darwin",)):
            with self.subTest(args=args):
                result = self.run_script("scripts/package", "Darwin", "x86_64", *args)
                self.assertEqual(result.returncode, 97)
                self.assertIn("for macos-x86_64", result.stdout)
                self.assertIn("unexpected build", result.stderr)

    def test_supported_installers_still_try_download(self):
        for os_name, arch in (("Darwin", "arm64"), ("Linux", "x86_64"), ("Linux", "aarch64")):
            with self.subTest(os=os_name, arch=arch):
                result = self.run_script("distribution/install.sh", os_name, arch)
                self.assertEqual(result.returncode, 1)
                self.assertIn("unexpected download", result.stderr)
                self.assertNotIn("Intel Macs", result.stderr)

    def package_checkout(self):
        checkout = Path(tempfile.mkdtemp(prefix="checkout-", dir=self.bin))
        for path in [
            "Cargo.toml", "LICENSE", "THIRD-PARTY-NOTICES", "scripts/package",
            ".cargo/config.toml",
            "assets/icons/herdr-ui-icon-clean.svg", "assets/icons/Herdr.icns",
        ]:
            destination = checkout / path
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / path, destination)
        sign = checkout / "scripts/mac-sign"
        sign.write_text(f'#!/bin/sh\ntouch "{checkout}/sign-called"\n')
        sign.chmod(0o755)
        return checkout

    def test_packages_keep_full_notices_on_every_platform(self):
        checkout = self.package_checkout()
        release = checkout / "target/release"
        release.mkdir(parents=True)
        for name in ("herdr-app", "herdr-app.exe"):
            (release / name).write_bytes(b"test binary")
        for triple in ("x86_64-apple-darwin", "aarch64-apple-darwin"):
            mac_release = checkout / f"target/{triple}/release"
            mac_release.mkdir(parents=True)
            (mac_release / "herdr-app").write_bytes(b"test binary")
        self.stub("cargo", "exit 0")
        self.stub("lipo", 'case "$TEST_ARCH" in arm64 | aarch64) echo arm64 ;; *) echo x86_64 ;; esac')
        self.stub("otool", f'printf "cmd LC_BUILD_VERSION\\nminos {MIN_MACOS}\\n"')
        for os_name, arch, target, prefix in (
            ("Linux", "x86_64", "linux-x86_64", "herdr-app"),
            ("Darwin", "arm64", "macos-aarch64", "Herdr.app/Contents/Resources"),
            ("Darwin", "x86_64", "macos-x86_64", "Herdr.app/Contents/Resources"),
            ("MINGW64_NT", "x86_64", "windows-x86_64", "herdr-app"),
        ):
            with self.subTest(target=target):
                result = self.run_script(checkout / "scripts/package", os_name, arch)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                with tarfile.open(checkout / f"dist/herdr-app-{target}.tar.gz") as archive:
                    for name in ("LICENSE", "THIRD-PARTY-NOTICES"):
                        self.assertEqual(
                            archive.extractfile(f"{prefix}/{name}").read(),
                            (ROOT / name).read_bytes(),
                        )
        self.stub("cargo", 'echo "unexpected build" >&2; exit 97')
        for name in ("LICENSE", "THIRD-PARTY-NOTICES"):
            notice = checkout / name
            original = notice.read_bytes()
            for state in ("empty", "missing"):
                with self.subTest(notice=name, state=state):
                    if state == "empty":
                        notice.write_bytes(b"")
                    else:
                        notice.unlink()
                    result = self.run_script(checkout / "scripts/package", "Linux", "x86_64")
                    self.assertEqual(result.returncode, 1)
                    self.assertIn(f"{name} is missing or empty", result.stderr)
                    self.assertNotIn("unexpected build", result.stderr)
            notice.write_bytes(original)

    def native_mac_package(self, rust_host, binary_arch, *args, deployment=MIN_MACOS, shell_arch="x86_64"):
        checkout = self.package_checkout()
        source = checkout / "tiny.c"
        source.write_text("int main(void) { return 0; }\n")
        binary = checkout / "tiny"
        subprocess.run(["xcrun", "clang", "-arch", binary_arch, f"-mmacosx-version-min={deployment}",
                        "-Wl,-no_adhoc_codesign", str(source), "-o", str(binary)],
                       capture_output=True, text=True, check=True)
        self.stub("cargo", f'''printf '%s\\n' "$@" > "{checkout}/cargo-args"
printf '%s' "$MACOSX_DEPLOYMENT_TARGET" > "{checkout}/cargo-min"
target=
while [ "$#" -gt 0 ]; do
  if [ "$1" = --target ]; then target=$2; shift; fi
  shift
done
destination="target/${{target:+$target/}}release"
mkdir -p "$destination"
cp "{binary}" "$destination/herdr-app"''')
        result = self.run_script(checkout / "scripts/package", "Darwin", shell_arch, *args, rust_host=rust_host)
        return checkout, result

    @unittest.skipUnless(NATIVE_MAC, "requires macOS Mach-O tools and Xcode")
    def test_mac_packages_match_rust_host_and_explicit_targets_under_rosetta(self):
        for host, shell_arch, actual_arch, args, target in (
            ("aarch64-apple-darwin", "x86_64", "arm64", (), "macos-aarch64"),
            ("x86_64-apple-darwin", "x86_64", "x86_64", (), "macos-x86_64"),
            ("aarch64-apple-darwin", "arm64", "arm64", (), "macos-aarch64"),
            ("x86_64-apple-darwin", "arm64", "x86_64", (), "macos-x86_64"),
            ("aarch64-apple-darwin", "x86_64", "x86_64", ("x86_64-apple-darwin",), "macos-x86_64"),
        ):
            with self.subTest(host=host, shell_arch=shell_arch, args=args):
                checkout, result = self.native_mac_package(host, actual_arch, *args, shell_arch=shell_arch)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                triple = "x86_64-apple-darwin" if target == "macos-x86_64" else "aarch64-apple-darwin"
                self.assertEqual((checkout / "cargo-args").read_text().splitlines()[-2:], ["--target", triple])
                self.assertEqual((checkout / "cargo-min").read_text(), MIN_MACOS)
                with tarfile.open(checkout / f"dist/herdr-app-{target}.tar.gz") as archive:
                    plist = plistlib.loads(archive.extractfile("Herdr.app/Contents/Info.plist").read())
                    self.assertEqual(plist.get("LSMinimumSystemVersion"), MIN_MACOS)
                    binary = checkout / "packed-binary"
                    binary.write_bytes(archive.extractfile("Herdr.app/Contents/MacOS/Herdr").read())
                arch = subprocess.check_output(["lipo", "-archs", str(binary)], text=True).strip()
                self.assertEqual(arch, actual_arch)

    @unittest.skipUnless(NATIVE_MAC, "requires macOS Mach-O tools and Xcode")
    def test_mac_package_rejects_wrong_architecture_before_signing(self):
        checkout, result = self.native_mac_package("x86_64-apple-darwin", "arm64")
        self.assertEqual(result.returncode, 1)
        self.assertIn("expected x86_64 Mach-O, found arm64", result.stderr)
        self.assertFalse((checkout / "sign-called").exists())
        self.assertFalse((checkout / "dist/herdr-app-macos-x86_64.tar.gz").exists())

    @unittest.skipUnless(NATIVE_MAC, "requires macOS Mach-O tools and Xcode")
    def test_mac_package_rejects_a_stale_lower_deployment_target_before_signing(self):
        checkout, result = self.native_mac_package("x86_64-apple-darwin", "x86_64", deployment="12.0")
        self.assertEqual(result.returncode, 1)
        self.assertIn(f"expected macOS {MIN_MACOS} deployment target, found 12.0", result.stderr)
        self.assertFalse((checkout / "sign-called").exists())
        self.assertFalse((checkout / "dist/herdr-app-macos-x86_64.tar.gz").exists())

    def test_supported_packages_still_try_build(self):
        for os_name, arch in (("Darwin", "arm64"), ("Linux", "x86_64"), ("Linux", "aarch64")):
            with self.subTest(os=os_name, arch=arch):
                result = self.run_script("scripts/package", os_name, arch)
                self.assertEqual(result.returncode, 97)
                self.assertIn("unexpected build", result.stderr)
                self.assertNotIn("Intel Macs", result.stderr)


if __name__ == "__main__":
    unittest.main()
