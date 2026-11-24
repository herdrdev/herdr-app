"""Require every release tarball before the workflow uploads any app artifact."""
import hashlib
import json
from pathlib import Path
import sys
from urllib.parse import urlsplit

from dmg_manifest import version_key

TARGETS = ("linux-aarch64", "linux-x86_64", "macos-aarch64", "macos-x86_64", "windows-x86_64")


def manifest(version, core, directory, base):
    version_key(version)
    version_key(core)
    url = urlsplit(base)
    if url.scheme != "https" or not url.hostname or url.username or url.password or url.query or url.fragment:
        raise ValueError("expected an HTTPS release base without credentials, query or fragment")
    directory = Path(directory)
    files = {f.name for f in directory.glob("herdr-app-*.tar.gz")}
    expected = {f"herdr-app-{target}.tar.gz" for target in TARGETS}
    if files != expected:
        raise ValueError(f"incomplete release tarballs; missing={sorted(expected - files)}, unexpected={sorted(files - expected)}")
    targets = {}
    for target in TARGETS:
        name = f"herdr-app-{target}.tar.gz"
        path = directory / name
        if path.is_symlink() or not path.is_file() or not path.stat().st_size:
            raise ValueError(f"missing, empty or symlinked tarball: {name}")
        targets[target] = {"url": f"{base.rstrip('/')}/app/{version}/{name}", "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
    return {"version": version, "core_min": core, "notes": f"Herdr {version}", "targets": targets}


if __name__ == "__main__":
    try:
        version, core, directory, base = sys.argv[1:]
        print(json.dumps(manifest(version, core, directory, base), indent=2))
    except (ValueError, OSError) as error:
        sys.exit(f"release manifest: {error}")
