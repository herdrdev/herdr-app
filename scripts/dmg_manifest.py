"""Describe a complete pair of signed DMGs; the workflow publishes this last."""
import hashlib
import json
from pathlib import Path
import re
import sys
from urllib.parse import urlsplit


def version_key(version):
    match = re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z.-]+))?(?:\+([0-9A-Za-z.-]+))?", version)
    if not match:
        raise ValueError("expected a release version")
    major, minor, patch, pre, build = match.groups()
    identifiers = pre.split(".") if pre else []
    if any(not part or (part.isdigit() and len(part) > 1 and part.startswith("0")) for part in identifiers):
        raise ValueError("invalid prerelease version")
    if build and any(not part for part in build.split(".")):
        raise ValueError("invalid build version")
    return (int(major), int(minor), int(patch), pre is None,
            tuple((0, int(part)) if part.isdigit() else (1, part) for part in identifiers))


def manifest(version, directory, base):
    version_key(version)
    url = urlsplit(base)
    if url.scheme != "https" or not url.hostname or url.username or url.password or url.query or url.fragment:
        raise ValueError("expected an HTTPS release base without credentials, query or fragment")
    targets = {}
    for target in ("macos-aarch64", "macos-x86_64"):
        name = f"Herdr-{target}.dmg"
        path = Path(directory) / name
        if path.is_symlink() or not path.is_file() or not path.stat().st_size:
            raise ValueError(f"missing or empty DMG: {name}")
        targets[target] = {
            "url": f"{base.rstrip('/')}/app/{version}/{name}",
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        }
    return {"version": version, "targets": targets}


if __name__ == "__main__":
    try:
        version, directory, base = sys.argv[1:]
        print(json.dumps(manifest(version, directory, base), indent=2))
    except (ValueError, OSError) as error:
        sys.exit(f"dmg manifest: {error}")
