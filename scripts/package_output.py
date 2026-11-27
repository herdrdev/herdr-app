"""Publish a verified local tarball/receipt under scripts/package's target lock.

The two regular filenames remain compatible. Readers must wait for package's
successful exit (or respect its lock); two filenames cannot be one atomic read.
An interrupted transaction keeps the lock and recoverable previous files.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import sys
import tarfile
import tempfile


def digest(file):
    value = hashlib.sha256()
    with file.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def verify(archive, receipt):
    for file in (archive, receipt):
        if file.is_symlink() or not file.is_file():
            raise ValueError(f"expected a regular output: {file}")
    data = json.loads(receipt.read_text())
    if digest(archive) != data["archive_sha256"]:
        raise ValueError("archive does not match its receipt")
    with tarfile.open(archive) as tar:
        member = tar.getmember(data["archive_member"])
        if not member.isfile():
            raise ValueError("archived executable is not a regular file")
        value = hashlib.sha256()
        with tar.extractfile(member) as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                value.update(chunk)
    if value.hexdigest() != data["packaged_executable_sha256"]:
        raise ValueError("archived executable does not match its receipt")
    return data


def identity(path):
    if not path.exists() and not path.is_symlink():
        return None
    info = path.lstat()
    return [info.st_dev, info.st_ino]


def publish(archive, receipt, directory, lock):
    archive, receipt, directory, lock = map(Path, (archive, receipt, directory, lock))
    base = archive.name[:-7] if archive.name.endswith(".tar.gz") else ""
    if (not re.fullmatch(r"herdr-app-(linux|macos|windows)-(x86_64|aarch64)", base)
            or receipt.name != base + ".build.json"
            or lock.resolve() != (directory / ("." + base + ".lock")).resolve()
            or not lock.is_dir()):
        raise ValueError("expected the output pair and its held target lock")
    verify(archive, receipt)
    marker = lock / "transaction.json"
    if marker.exists():
        raise ValueError(f"unfinished publication: inspect {marker}")
    stage = Path(tempfile.mkdtemp(prefix="." + base + ".publish-", dir=directory))
    # This marker precedes every output mutation. Even SIGKILL of this child
    # makes the shell retain the target lock instead of admitting another run.
    marker.write_text(json.dumps({"stage": str(stage.resolve())}) + "\n")
    publishing = False
    entries = []
    try:
        for source, label in ((archive, "archive"), (receipt, "receipt")):
            new = stage / source.name
            shutil.copy2(source, new)
            dest, backup = directory / source.name, stage / ("previous-" + label)
            old_id = identity(dest)
            old_sha = None
            if old_id is not None:
                if dest.is_symlink() or not dest.is_file():
                    raise ValueError(f"refusing non-regular previous output: {dest}")
                old_sha = digest(dest)
                shutil.copy2(dest, backup)
                if digest(backup) != old_sha or identity(dest) != old_id:
                    raise ValueError("previous output changed while backing it up")
            entries.append({"name": source.name, "new": str(new), "new_id": identity(new),
                            "new_sha": digest(new), "old_id": old_id, "old_sha": old_sha,
                            "backup": str(backup)})
        verify(stage / archive.name, stage / receipt.name)
        (stage / "recovery.json").write_text(json.dumps(entries, indent=2) + "\n")
        publishing = True
        for item in entries:
            os.replace(item["new"], directory / item["name"])
        verify(directory / archive.name, directory / receipt.name)
        if any(digest(directory / item["name"]) != item["new_sha"] for item in entries):
            raise ValueError("published pair changed before verification")
    except BaseException as error:
        if publishing:
            try:
                for item in entries:
                    dest, backup = directory / item["name"], Path(item["backup"])
                    current = identity(dest)
                    if current == item["old_id"]:
                        if current is None or digest(dest) == item["old_sha"]:
                            continue
                    if current not in (None, item["new_id"]):
                        raise ValueError(f"another writer changed {dest}; it was not removed")
                    if item["old_id"] is None:
                        if current is not None:
                            dest.unlink()
                    else:
                        os.replace(backup, dest)
                        if digest(dest) != item["old_sha"]:
                            raise ValueError(f"cannot verify restored output: {dest}")
            except BaseException as recovery:
                raise RuntimeError(f"publication recovery required: inspect {stage} and {lock}; do not retry or remove the lock before recovering the previous pair ({recovery})") from error
        marker.unlink()
        shutil.rmtree(stage)
        raise
    marker.unlink()
    # Publication is committed and verified. A leftover private backup must
    # not turn a valid pair into a reported failed/partial publication.
    shutil.rmtree(stage, ignore_errors=True)


if __name__ == "__main__":
    def interrupted(signum, _frame):
        raise InterruptedError(f"publication interrupted by signal {signum}")
    for name in ("SIGINT", "SIGTERM", "SIGHUP"):
        if hasattr(signal, name):
            signal.signal(getattr(signal, name), interrupted)
    try:
        publish(*sys.argv[1:])
    except (OSError, ValueError, RuntimeError, KeyError, tarfile.TarError) as error:
        sys.exit(f"package: {error}")
