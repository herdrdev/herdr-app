"""Distribution architecture guards, without downloads or release builds.

Run: uv run --no-project python -m unittest discover -s scripts -p test_distribution.py
"""

import hashlib
import os
import plistlib
import re
import shlex
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
        self.system = self.bin / "system"
        self.system.mkdir()
        self.stub("uname", 'case "$1" in -s) echo "$TEST_OS";; -m) echo "$TEST_ARCH";; esac')
        self.stub("curl", 'echo "unexpected download" >&2; exit 97')
        self.stub("cargo", 'echo "unexpected build" >&2; exit 97')
        self.stub("sysctl", 'exit 1')
        self.system_sysctl('exit 1')
        self.stub("sw_vers", 'echo "${TEST_MACOS_VERSION:-15.0}"')
        self.stub("rustc", 'printf "host: %s\\n" "$TEST_RUST_HOST"')

    def stub(self, name, body):
        path = self.bin / name
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(0o755)

    def system_sysctl(self, body):
        path = self.system / "sysctl"
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(0o755)

    def run_script(self, script, os_name, arch, *args, macos_version="15.0", rust_host=None, extra_env=None):
        rust_arch = "aarch64" if arch in ("arm64", "aarch64") else "x86_64"
        env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}", TEST_OS=os_name, TEST_ARCH=arch,
                   TEST_MACOS_VERSION=macos_version, TEST_RUST_HOST=rust_host or f"{rust_arch}-apple-darwin",
                   RUSTC=str(self.bin / "rustc"), HOME=str(self.bin / "home"), TMPDIR=str(self.bin))
        env.update(extra_env or {})
        entry = ROOT / script
        if entry == ROOT / "distribution/install.sh":
            # Model the OS's absolute sysctl independently of the caller PATH.
            entry = self.bin / "install-entry.sh"
            entry.write_text((ROOT / script).read_text().replace("/usr/sbin/sysctl", str(self.system / "sysctl")))
            entry.chmod(0o755)
        return subprocess.run(
            [str(entry), *args], cwd=ROOT, env=env,
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
                    self.system_sysctl(f'[ "$2" = "{key}" ] && echo 1')
                    self.check_mac_installer_target(arch, "macos-aarch64")

    def test_rosetta_detection_does_not_use_caller_path(self):
        self.system_sysctl('[ "$2" = sysctl.proc_translated ] && echo 1')
        (self.bin / "sysctl").unlink()
        self.check_mac_installer_target("x86_64", "macos-aarch64")
        self.stub("sysctl", "echo 0")
        self.check_mac_installer_target("x86_64", "macos-aarch64")
        self.system_sysctl("echo 0")
        self.stub("sysctl", "echo 1")
        self.check_mac_installer_target("x86_64", "macos-x86_64")

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

    def mac_installer(self, signature, status, verify_status=0, arch="arm64", *, package_arch=None, move_failure=None, core="existing", different_device=False, package_description=None, native_binary=None, extra_env=None):
        home = self.bin / "home"
        (home / ".local/bin").mkdir(parents=True)
        existing_core = home / ".local/bin/herdr-core"
        if core == "existing":
            existing_core.write_bytes(b"user's existing Core")
        elif core == "dangling":
            existing_core.symlink_to("user-selected-missing-core")
        source = self.bin / "source/Herdr.app/Contents"
        source.mkdir(parents=True)
        plist = b"signed release plist must stay unchanged"
        (source / "Info.plist").write_bytes(plist)
        if native_binary is not None:
            (source / "MacOS").mkdir()
            shutil.copy2(native_binary, source / "MacOS/Herdr")
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
case $url in
  */latest.json) cp "{manifest}" "$out" ;;
  release) cp "{archive}" "$out" ;;
  */core/install.sh) echo "core download attempted" >> "{self.bin}/core-attempt"; printf 'touch "{self.bin}/partial-core-executed"\\n' > "$out"; exit 22 ;;
  *) exit 97 ;;
esac''')
        installed = home / "Applications/Herdr.app/Contents/Info.plist"
        installed.parent.mkdir(parents=True)
        installed.write_bytes(b"previous installed app")
        self.previous_inode = installed.parent.parent.stat().st_ino
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
        description = package_description or "Mach-O 64-bit executable " + (package_arch or ("x86_64" if arch == "x86_64" else "arm64"))
        self.stub("file", "printf '%s\\n' " + shlex.quote(description))
        self.stub("ditto", 'cp -R "$1" "$2"')
        self.stub("mac-stat", f'''exec "{sys.executable}" - "{home}/Applications/Herdr.app" "{int(different_device)}" "$@" <<'PY'
import os, sys
path = sys.argv[-1]
value = os.lstat(path)
device = value.st_dev + (int(sys.argv[2]) if path == sys.argv[1] else 0)
print(str(device) + ":" + str(value.st_ino))
PY''')
        if move_failure:
            self.stub("mv", f'''echo "$1 -> $2" >> "{self.bin}/moves"
case "$2" in
  "{home}/Applications/Herdr.app")
    case "$1" in
      */previous) {"exit 74" if move_failure == "restore" else ":"} ;;
      *) exit 73 ;;
    esac ;;
esac
exec /bin/mv "$@"''')
        # Remap only the system tools in a scratch copy; production uses absolute paths.
        script = (ROOT / "distribution/install.sh").read_text()
        self.assertNotIn("\n    codesign ", script)
        script = script.replace("/usr/bin/codesign", str(self.bin / "codesign"))
        script = script.replace("/usr/libexec/PlistBuddy", str(self.bin / "PlistBuddy"))
        tools = [("/usr/sbin/sysctl", self.system / "sysctl"),
                 ("/usr/bin/ditto", self.bin / "ditto"),
                 ("/usr/bin/stat", self.bin / "mac-stat")]
        if native_binary is None:
            tools.append(("/usr/bin/file", self.bin / "file"))
        for absolute, stub in tools:
            script = script.replace(absolute, str(stub))
        self.stub("lsregister", "exit 0")
        script = script.replace("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
                                str(self.bin / "lsregister"))
        installer = self.bin / "install.sh"
        installer.write_text(script)
        env = dict(os.environ, PATH=f"{self.bin}:{Path(sys.executable).parent}:/usr/bin:/bin:/usr/sbin:/sbin", TEST_OS="Darwin",
                   TEST_ARCH=arch, HOME=str(home))
        env.update(extra_env or {})
        result = subprocess.run(["sh", str(installer)], cwd=ROOT, env=env,
                                capture_output=True, text=True, check=False)
        self.installed = installed
        self.installer_home = home
        return result, installed.read_bytes() if installed.exists() else None, log.read_text().splitlines() if log.exists() else [], plist_log.exists(), plist

    def test_final_move_failure_restores_the_exact_previous_app(self):
        signature = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, installed, *_ = self.mac_installer(signature, 0, arch="x86_64", move_failure="final")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(installed, b"previous installed app")
        self.assertEqual(self.installed.parent.parent.stat().st_ino, self.previous_inode)
        self.assertIn("previous app restored", result.stderr)
        moves = (self.bin / "moves").read_text().splitlines()
        self.assertTrue(any("/.herdr-install." in line and "/new -> " in line for line in moves))
        self.assertTrue(any("/previous -> " in line for line in moves))
        self.assertFalse((self.installer_home / "Applications/.herdr-app-install.lock").exists())

    def test_restore_failure_preserves_backup_and_blocks_another_install(self):
        signature = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, *_ = self.mac_installer(signature, 0, arch="x86_64", move_failure="restore")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Do not retry before recovering", result.stderr)
        backups = list((self.installer_home / "Applications").glob(".herdr-install.*/previous/Contents/Info.plist"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), b"previous installed app")
        self.assertEqual(backups[0].parent.parent.stat().st_ino, self.previous_inode)
        self.assertTrue((self.installer_home / "Applications/.herdr-app-install.lock").is_dir())

    def test_app_on_another_device_is_not_moved(self):
        signature = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, installed, *_ = self.mac_installer(signature, 0, arch="x86_64", different_device=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("another filesystem", result.stderr)
        self.assertEqual(installed, b"previous installed app")
        self.assertEqual(self.installed.parent.parent.stat().st_ino, self.previous_inode)
        self.assertFalse((self.installer_home / "Applications/.herdr-app-install.lock").exists())

    def test_wrong_package_architecture_preserves_previous_app_before_signing(self):
        signature = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, installed, calls, *_ = self.mac_installer(signature, 0, arch="x86_64", package_arch="arm64")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(installed, b"previous installed app")
        self.assertIn("expected x86_64 app, found arm64", result.stderr)
        self.assertEqual(calls, [])

    def test_universal_payload_is_not_mistaken_for_either_thin_architecture(self):
        signature = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, installed, calls, *_ = self.mac_installer(signature, 0, arch="x86_64",
            package_description="Mach-O universal binary with 2 architectures: x86_64 arm64")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsupported app binary", result.stderr)
        self.assertEqual(installed, b"previous installed app")
        self.assertEqual(calls, [])

    @unittest.skipUnless(NATIVE_MAC, "requires Xcode only to create the test fixture")
    def test_installer_binary_inspection_needs_no_selected_developer_tools(self):
        source, binary = self.bin / "tiny.c", self.bin / "tiny"
        source.write_text("int main(void) { return 0; }\n")
        subprocess.run(["xcrun", "clang", "-target", "x86_64-apple-macos13", str(source), "-o", str(binary)],
                       capture_output=True, text=True, check=True)
        signature = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, installed, _, _, plist = self.mac_installer(signature, 0, arch="x86_64", native_binary=binary,
            extra_env={"DEVELOPER_DIR": str(self.bin / "nonexistent-sdk")})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(installed, plist)

    def test_core_download_failure_is_actionable_and_never_executes_partial_script(self):
        signature = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, *_ = self.mac_installer(signature, 0, arch="x86_64", core="missing")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.bin / "core-attempt").exists())
        self.assertIn("herdr-core download failed", result.stderr)
        self.assertIn("Open Herdr", result.stderr)
        self.assertFalse((self.bin / "partial-core-executed").exists())

    def test_existing_or_dangling_core_is_never_reinstalled(self):
        signature = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, *_ = self.mac_installer(signature, 0, core="dangling")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.bin / "core-attempt").exists())
        self.assertEqual(os.readlink(self.installer_home / ".local/bin/herdr-core"), "user-selected-missing-core")

    def test_existing_core_bytes_are_preserved(self):
        signature = (ROOT / "scripts/fixtures/codesign/developer-id.txt").read_text()
        result, *_ = self.mac_installer(signature, 0)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.bin / "core-attempt").exists())
        self.assertEqual((self.installer_home / ".local/bin/herdr-core").read_bytes(), b"user's existing Core")

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
            "Cargo.toml", "LICENSE", "THIRD-PARTY-NOTICES", "scripts/package", "scripts/package_output.py",
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

    def program(self, path, version=None):
        version = version or re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M).group(1)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f'#!/bin/sh\nprintf "%s\\n" "herdr-app {version}"\n')
        path.chmod(0o755)

    def cargo_report(self, path=None, *, report=True, external=False, fresh=False, wrong_package=False):
        fixed = repr(str(path)) if path else "None"
        self.stub("cargo", f'''exec "{sys.executable}" - "$@" <<'PY'
import json, os, pathlib, re, shutil, sys
a = sys.argv[1:]
manifest = pathlib.Path('Cargo.toml').resolve()
version = re.search(r'^version = "([^"]+)"', manifest.read_text(), re.M).group(1)
package = {{'id':'fixture-herdr-app@'+version,'name':'herdr-app','version':version,'manifest_path':str(manifest)}}
if a[0] == 'metadata':
    print(json.dumps({{'packages':[package]}}))
    sys.exit(0)
pathlib.Path('cargo-args').write_text('\\n'.join(a))
pathlib.Path('cargo-min').write_text(os.environ.get('MACOSX_DEPLOYMENT_TARGET',''))
triple = a[a.index('--target')+1] if '--target' in a else None
windows = 'windows' in (triple or '') or os.environ['TEST_OS'].startswith(('MINGW', 'MSYS', 'CYGWIN'))
template = pathlib.Path({fixed}) if {fixed} else pathlib.Path('target') / (triple or '') / 'release' / ('herdr-app.exe' if windows else 'herdr-app')
path = template if {external!r} or '--target-dir' not in a else pathlib.Path(a[a.index('--target-dir')+1]) / (triple or '') / 'release' / template.name
path.parent.mkdir(parents=True,exist_ok=True)
if path != template: shutil.copy2(template,path)
if {report!r}:
    print(json.dumps({{'reason':'compiler-artifact','package_id':'wrong-package' if {wrong_package!r} else package['id'],'manifest_path':str(manifest),'fresh':{fresh!r},'target':{{'name':'herdr-app','kind':['bin']}},'profile':{{'test':False}},'executable':str(path.resolve())}}))
PY''')

    def test_packages_keep_full_notices_on_every_platform(self):
        checkout = self.package_checkout()
        release = checkout / "target/release"
        release.mkdir(parents=True)
        for name in ("herdr-app", "herdr-app.exe"):
            self.program(release / name)
        for triple in ("x86_64-apple-darwin", "aarch64-apple-darwin"):
            mac_release = checkout / f"target/{triple}/release"
            mac_release.mkdir(parents=True)
            self.program(mac_release / "herdr-app")
        self.cargo_report()
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

    def test_package_uses_the_cargo_artifact_instead_of_a_stale_default_target(self):
        checkout = self.package_checkout()
        stale = checkout / "target/x86_64-apple-darwin/release/herdr-app"
        fresh = checkout / "custom target with spaces/x86_64-apple-darwin/release/herdr-app"
        self.program(stale, "0.0.1")
        self.program(fresh)
        self.cargo_report(fresh)
        self.stub("lipo", "echo x86_64")
        self.stub("otool", f'printf "cmd LC_BUILD_VERSION\\nminos {MIN_MACOS}\\n"')
        result = self.run_script(checkout / "scripts/package", "Darwin", "x86_64", extra_env={"CARGO_TARGET_DIR":str(fresh.parent.parent.parent)})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        with tarfile.open(checkout / "dist/herdr-app-macos-x86_64.tar.gz") as archive:
            self.assertEqual(archive.extractfile("Herdr.app/Contents/MacOS/Herdr").read(), fresh.read_bytes())
        self.assertIn("0.0.1", stale.read_text())

    def test_package_rejects_a_reported_old_version_before_signing(self):
        checkout = self.package_checkout()
        hardware_arm = sys.platform == "darwin" and subprocess.run(
            ["/usr/sbin/sysctl", "-n", "hw.optional.arm64"], capture_output=True, text=True,
        ).stdout.strip() == "1"
        arch = "arm64" if hardware_arm else "x86_64"
        rust_arch = "aarch64" if hardware_arm else "x86_64"
        stale = checkout / f"target/{rust_arch}-apple-darwin/release/herdr-app"
        self.program(stale, "0.0.1")
        self.cargo_report(stale)
        self.stub("lipo", f"echo {arch}")
        self.stub("otool", f'printf "cmd LC_BUILD_VERSION\\nminos {MIN_MACOS}\\n"')
        result = self.run_script(checkout / "scripts/package", "Darwin", arch)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected herdr-app", result.stderr)
        self.assertIn("found herdr-app 0.0.1", result.stderr)
        self.assertFalse((checkout / "sign-called").exists())
        self.assertFalse((checkout / f"dist/herdr-app-macos-{rust_arch}.tar.gz").exists())

    def test_package_requires_a_current_cargo_executable_event(self):
        checkout = self.package_checkout()
        stale = checkout / "target/x86_64-apple-darwin/release/herdr-app"
        self.program(stale)
        self.cargo_report(stale, report=False)
        self.stub("lipo", "echo x86_64")
        self.stub("otool", f'printf "cmd LC_BUILD_VERSION\\nminos {MIN_MACOS}\\n"')
        result = self.run_script(checkout / "scripts/package", "Darwin", "x86_64")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Cargo did not report exactly one", result.stderr)
        self.assertFalse((checkout / "sign-called").exists())

    def test_unfinished_publication_keeps_its_lock_and_blocks_a_new_build(self):
        checkout = self.package_checkout()
        binary = checkout / "fixture/herdr-app"
        self.program(binary)
        self.cargo_report(binary)
        self.stub("lipo", "echo x86_64")
        self.stub("otool", f'printf "cmd LC_BUILD_VERSION\\nminos {MIN_MACOS}\\n"')
        helper = checkout / "scripts/package_output.py"
        helper.write_text('import pathlib,sys\n(pathlib.Path(sys.argv[4])/"transaction.json").write_text("interrupted fixture")\nsys.exit(86)\n')
        failed = self.run_script(checkout / "scripts/package", "Darwin", "x86_64")
        self.assertNotEqual(failed.returncode, 0)
        lock = checkout / "dist/.herdr-app-macos-x86_64.lock"
        self.assertTrue((lock / "transaction.json").is_file())
        self.assertIn("unfinished publication", failed.stderr)
        self.stub("cargo", 'echo "must not build" >&2; exit 97')
        retry = self.run_script(checkout / "scripts/package", "Darwin", "x86_64")
        self.assertNotEqual(retry.returncode, 0)
        self.assertIn("target is locked", retry.stderr)
        self.assertNotIn("must not build", retry.stderr)

    def test_package_rejects_shared_cached_or_other_package_artifacts_before_signing(self):
        for option in ("external", "fresh", "wrong_package"):
            with self.subTest(option=option):
                checkout = self.package_checkout()
                binary = checkout / "shared/herdr-app"
                self.program(binary)
                self.cargo_report(binary, **{option: True})
                self.stub("lipo", "echo x86_64")
                self.stub("otool", f'printf "cmd LC_BUILD_VERSION\\nminos {MIN_MACOS}\\n"')
                result = self.run_script(checkout / "scripts/package", "Darwin", "x86_64")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("freshly compiled executable from this package", result.stderr)
                self.assertFalse((checkout / "sign-called").exists())

    def native_mac_package(self, rust_host, binary_arch, *args, deployment=MIN_MACOS, shell_arch="x86_64"):
        checkout = self.package_checkout()
        source = checkout / "tiny.c"
        version = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M).group(1)
        source.write_text(f'#include <stdio.h>\nint main(void) {{ puts("herdr-app {version}"); return 0; }}\n')
        binary = checkout / "tiny"
        subprocess.run(["xcrun", "clang", "-arch", binary_arch, f"-mmacosx-version-min={deployment}",
                        "-Wl,-no_adhoc_codesign", str(source), "-o", str(binary)],
                       capture_output=True, text=True, check=True)
        # Like rustc's linked executable, it must be runnable before the bundle
        # signing step. Apple Silicon requires at least an ad-hoc code signature.
        subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", str(binary)],
                       capture_output=True, text=True, check=True)
        self.cargo_report(binary)
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
                hardware_arm = subprocess.run(["/usr/sbin/sysctl", "-n", "hw.optional.arm64"], capture_output=True, text=True).stdout.strip() == "1"
                native_arch = "arm64" if hardware_arm else "x86_64"
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(f"for {target}", result.stdout)
                if actual_arch != native_arch:
                    self.assertIn(f"not executing {actual_arch} on {native_arch}", result.stdout)
                else:
                    self.assertNotIn("not executing", result.stdout)
                import json
                receipt = json.loads((checkout / f"dist/herdr-app-{target}.build.json").read_text())
                self.assertEqual(receipt["version_verification"], "isolated-cargo-metadata" if actual_arch != native_arch else "execute")
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
