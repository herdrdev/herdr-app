"""Publish immutable DMG releases and advance latest objects with S3 preconditions.

Uses the existing release environment's AWS CLI authorization. No public-URL
downloads or credential/configuration reads. If conditional writes are unavailable,
publication fails; there is no unconditional-write fallback.
"""
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from dmg_manifest import manifest, version_key


class Conflict(RuntimeError):
    pass


@dataclass
class Object:
    body: bytes
    etag: str
    metadata: dict


class S3:
    def __init__(self, endpoint, bucket):
        self.args = ["aws", "--endpoint-url", endpoint, "s3api"]
        self.bucket = bucket

    def call(self, operation, key=None, *args):
        command = [*self.args, operation, "--bucket", self.bucket, "--output", "json", "--no-cli-pager"]
        if key is not None:
            command += ["--key", key]
        result = subprocess.run([*command, *args], capture_output=True, text=True, timeout=120)
        if result.returncode:
            code = re.search(r"\(([^)]+)\) when calling", result.stderr)
            code = code.group(1) if code else "CLI failure"
            if operation == "get-object" and code in ("NoSuchKey", "NotFound", "404"):
                return None
            if code in ("PreconditionFailed", "412", "ConditionalRequestConflict", "409"):
                raise Conflict(f"{operation}: {key}: {code}")
            raise RuntimeError(f"{operation}: {key}: {code}")
        return json.loads(result.stdout)

    def get(self, key):
        with tempfile.TemporaryDirectory(prefix="herdr-dmg-read-") as tmp:
            path = Path(tmp) / "object"
            data = self.call("get-object", key, str(path))
            return None if data is None else Object(path.read_bytes(), data["ETag"], data.get("Metadata", {}))

    def put(self, key, body, metadata, content_type, etag=None):
        with tempfile.TemporaryDirectory(prefix="herdr-dmg-put-") as tmp:
            path = Path(tmp) / "object"
            path.write_bytes(body)
            condition = ["--if-match", etag] if etag is not None else ["--if-none-match", "*"]
            self.call("put-object", key, "--body", str(path), "--metadata", json.dumps(metadata),
                      "--content-type", content_type, "--cache-control",
                      "no-cache" if "/latest.json" in key or key.count("/") == 1 else "public, max-age=31536000, immutable",
                      *condition)

    def keys(self):
        # AWS CLI automatically combines ListObjectsV2 pages. Used only to
        # identify the version of a pre-existing alias without our metadata.
        return [item["Key"] for item in self.call("list-objects-v2", None, "--prefix", "app/").get("Contents", [])]


def sha(body):
    return hashlib.sha256(body).hexdigest()


class Publisher:
    def __init__(self, store, version, source):
        version_key(version)
        if not re.fullmatch(r"[0-9a-f]{40}(?:[0-9a-f]{24})?", source):
            raise ValueError("expected source commit SHA")
        self.store, self.version, self.source = store, version, source

    def info(self, body, target):
        return {"version": self.version, "source": self.source, "target": target, "sha256": sha(body)}

    def immutable(self, key, body, info, content_type):
        old = self.store.get(key)
        if old is None:
            try:
                self.store.put(key, body, info, content_type)
                return
            except Conflict:
                old = self.store.get(key)
        if old is None or old.body != body:
            raise RuntimeError(f"immutable release conflict: {key}")

    def document(self, key):
        item = self.store.get(key)
        return None if item is None else json.loads(item.body)

    def check_reference(self, target, body):
        for key in (f"app/{self.version}/dmg.json", "app/dmg/latest.json"):
            doc = self.document(key)
            if doc and doc["version"] == self.version:
                if doc["targets"][target]["sha256"] != sha(body):
                    raise RuntimeError(f"published checksum conflict: {key}")

    def adopt(self, item, candidate, target):
        if item.metadata and item.metadata != self.info(item.body, target):
            raise RuntimeError("cannot reuse a DMG with conflicting provenance")
        if item.body == candidate:
            return candidate
        if item.metadata != self.info(item.body, target):
            raise RuntimeError("cannot reuse a DMG from another commit or unknown provenance")
        return item.body  # same commit, different timestamped signing: first writer wins

    def alias_version(self, key, item):
        version = item.metadata.get("version")
        if version:
            if item.metadata.get("sha256") != sha(item.body):
                raise RuntimeError(f"alias checksum conflict: {key}")
            return version
        # Migrate existing ARM aliases safely: verify bytes against actual
        # versioned history, rather than guessing from the tarball's latest.
        name = key.rsplit("/", 1)[-1]
        matches = []
        for stored in self.store.keys():
            parts = stored.split("/")
            if len(parts) == 3 and parts[0] == "app" and parts[2] == name:
                try:
                    matches.append((version_key(parts[1]), parts[1], stored))
                except ValueError:
                    pass
        for _, version, stored in sorted(matches, reverse=True):
            other = self.store.get(stored)
            if other and other.body == item.body:
                return version
        raise RuntimeError(f"cannot identify legacy alias version: {key}")

    def advance(self, key, body, info, content_type, document=False):
        for _ in range(8):
            old = self.store.get(key)
            if old is not None:
                version = json.loads(old.body)["version"] if document else self.alias_version(key, old)
                current, wanted = version_key(version), version_key(self.version)
                if current > wanted:
                    return  # an older tag retry must not roll latest back
                if current == wanted:
                    if version != self.version or old.body != body:
                        raise RuntimeError(f"same-version latest conflict: {key}")
                    return
            try:
                self.store.put(key, body, info, content_type, None if old is None else old.etag)
                return
            except Conflict:
                pass  # re-read the winner; never overwrite on a stale ETag
        raise RuntimeError(f"latest publication contention: {key}")

    def image(self, target, path):
        if target not in ("macos-aarch64", "macos-x86_64"):
            raise ValueError("unsupported DMG target")
        path = Path(path)
        body = path.read_bytes()
        if not body or path.is_symlink():
            raise ValueError("expected a nonempty regular DMG")
        key = f"app/{self.version}/Herdr-{target}.dmg"
        legacy = f"app/{self.version}/Herdr.dmg" if target == "macos-aarch64" else None
        old, previous = self.store.get(key), self.store.get(legacy) if legacy else None
        if old:
            body = self.adopt(old, body, target)
        elif previous:
            body = self.adopt(previous, body, target)
        self.check_reference(target, body)
        if previous and previous.body != body:
            raise RuntimeError("existing versioned ARM aliases disagree")
        if old is None:
            try:
                self.store.put(key, body, self.info(body, target), "application/x-apple-diskimage")
            except Conflict:
                winner = self.store.get(key)
                if winner is None:
                    raise RuntimeError("conditional DMG winner disappeared")
                body = self.adopt(winner, body, target)
                self.check_reference(target, body)
        if legacy:
            self.immutable(legacy, body, self.info(body, target), "application/x-apple-diskimage")
        # The retained Actions artifact and final manifest use committed bytes,
        # including when a retry rebuilt/signature-timestamped another image.
        if path.read_bytes() != body:
            path.write_bytes(body)
        for name in (f"Herdr-{target}.dmg", *(["Herdr.dmg"] if legacy else [])):
            self.advance(f"app/{name}", body, self.info(body, target), "application/x-apple-diskimage")

    def release(self, directory, base):
        doc = manifest(self.version, directory, base)
        for target, value in doc["targets"].items():
            keys = [f"app/{self.version}/Herdr-{target}.dmg"]
            if target == "macos-aarch64":
                keys.append(f"app/{self.version}/Herdr.dmg")
            for key in keys:
                item = self.store.get(key)
                if item is None or sha(item.body) != value["sha256"]:
                    raise RuntimeError(f"remote DMG checksum mismatch: {key}")
                if item.metadata.get("source", self.source) != self.source:
                    raise RuntimeError(f"remote source commit mismatch: {key}")
        body = (json.dumps(doc, indent=2) + "\n").encode()
        info = self.info(body, "manifest")
        self.immutable(f"app/{self.version}/dmg.json", body, info, "application/json")
        self.advance("app/dmg/latest.json", body, info, "application/json", document=True)


if __name__ == "__main__":
    try:
        mode, version, *args = sys.argv[1:]
        publisher = Publisher(S3(os.environ["ENDPOINT"], os.environ["BUCKET"]), version, os.environ["GITHUB_SHA"])
        if mode == "image":
            target, path = args
            publisher.image(target, path)
        elif mode == "manifest":
            directory, base = args
            publisher.release(directory, base)
        else:
            raise ValueError("expected image or manifest")
    except (ValueError, KeyError, RuntimeError, OSError, subprocess.TimeoutExpired) as error:
        sys.exit(f"DMG publication failed: {error}")
