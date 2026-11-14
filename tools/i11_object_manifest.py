#!/usr/bin/env python3
"""Offline I-11 object inventory snapshot/verification; no S3 or DB access."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import stat
import sys

MAX_MANIFEST_BYTES = 256 * 1024 * 1024
MAX_OBJECTS = 1_000_000
HEX_SHA256 = re.compile(r"[0-9a-f]{64}\Z")


class Refused(ValueError):
    """Failure that must not disclose private object paths."""


def safe_key(key: object) -> str:
    if not isinstance(key, str) or not key or len(key.encode("utf-8")) > 1024:
        raise Refused("invalid object key")
    if (key.startswith("/") or "\\" in key or any(ord(c) < 32 or ord(c) == 127 for c in key)
            or any(part in ("", ".", "..") for part in key.split("/"))):
        raise Refused("unsafe object key")
    return key


def root_directory(value: str) -> Path:
    root = Path(value)
    if not root.is_absolute() or root.is_symlink() or not root.is_dir() or root == Path("/"):
        raise Refused("absolute dedicated restore directory required")
    return root.resolve(strict=True)


def inventory(root: Path) -> list[dict[str, object]]:
    items = []
    def reject_scan_error(error: OSError) -> None:
        raise Refused("incomplete or unreadable object tree") from error

    for parent, dirs, files in os.walk(root, followlinks=False,
                                        onerror=reject_scan_error):
        for name in dirs:
            path = Path(parent) / name
            if path.is_symlink():
                raise Refused("symlinked directory refused")
        for name in files:
            path = Path(parent) / name
            rel = safe_key(path.relative_to(root).as_posix())
            try:
                if not stat.S_ISREG(path.lstat().st_mode):
                    raise Refused("non-regular object refused")
                fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                with os.fdopen(fd, "rb") as file:
                    before = os.fstat(file.fileno())
                    if not stat.S_ISREG(before.st_mode):
                        raise Refused("non-regular object refused")
                    sha = hashlib.sha256()
                    count = 0
                    for chunk in iter(lambda: file.read(1024 * 1024), b""):
                        sha.update(chunk)
                        count += len(chunk)
                    after = os.fstat(file.fileno())
                    if (count != before.st_size or before.st_size != after.st_size
                            or before.st_mtime_ns != after.st_mtime_ns):
                        raise Refused("object changed during scan")
            except OSError as error:
                raise Refused("unreadable or unsafe object") from error
            items.append({"key": rel, "size": count, "sha256": sha.hexdigest()})
            if len(items) > MAX_OBJECTS:
                raise Refused("object count limit exceeded")
    items.sort(key=lambda item: item["key"])
    if not items or sum(item["size"] for item in items) <= 0:
        raise Refused("missing or empty inventory")
    return items


def validate_manifest(raw: bytes) -> list[dict[str, object]]:
    try:
        data = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise Refused("invalid manifest JSON") from error
    if not isinstance(data, dict) or set(data) != {"schema_version", "created_utc", "objects"}:
        raise Refused("unexpected manifest fields")
    if type(data["schema_version"]) is not int or data["schema_version"] != 1:
        raise Refused("unsupported schema")
    ts = data["created_utc"]
    if not isinstance(ts, str) or not re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", ts):
        raise Refused("invalid snapshot timestamp")
    try:
        datetime.strptime(ts, "%Y-%m-%dT%H:%M:%SZ")
    except ValueError as error:
        raise Refused("invalid snapshot timestamp") from error
    entries = data["objects"]
    if not isinstance(entries, list) or not 0 < len(entries) <= MAX_OBJECTS:
        raise Refused("invalid number of manifest objects")
    previous = ""
    total_size = 0
    for entry in entries:
        if not isinstance(entry, dict) or set(entry) != {"key", "size", "sha256"}:
            raise Refused("invalid object manifest entry")
        key = safe_key(entry["key"])
        if key <= previous:
            raise Refused("duplicate or unsorted manifest object")
        previous = key
        if type(entry["size"]) is not int or entry["size"] < 0:
            raise Refused("invalid object size")
        total_size += entry["size"]
        if not isinstance(entry["sha256"], str) or not HEX_SHA256.fullmatch(entry["sha256"]):
            raise Refused("invalid object checksum")
    if total_size <= 0:
        raise Refused("empty manifest")
    return entries


def manifest_bytes(path: Path) -> bytes:
    if not path.is_file() or path.is_symlink():
        raise Refused("regular manifest file required")
    if path.stat().st_size > MAX_MANIFEST_BYTES:
        raise Refused("oversized manifest")
    raw = path.read_bytes()
    if len(raw) > MAX_MANIFEST_BYTES:
        raise Refused("oversized manifest")
    return raw


def snapshot(root: Path, output: Path) -> None:
    if not output.is_absolute() or output.is_symlink() or output.resolve().is_relative_to(root):
        raise Refused("manifest destination must be outside the source tree")
    objects = inventory(root)
    payload = {"schema_version": 1,
               "created_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
               "objects": objects}
    raw = (json.dumps(payload, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")
    if len(raw) > MAX_MANIFEST_BYTES:
        raise Refused("oversized manifest")
    old_umask = os.umask(0o077)
    try:
        with output.open("xb") as file:
            file.write(raw)
    finally:
        os.umask(old_umask)
    print("TOR_I11_MANIFEST_CREATED objects=%d bytes=%d sha256=%s" % (
        len(objects), sum(o["size"] for o in objects), hashlib.sha256(raw).hexdigest()))


def verify(root: Path, path: Path, expected_sha256: str) -> None:
    if not HEX_SHA256.fullmatch(expected_sha256):
        raise Refused("independent manifest SHA-256 required")
    raw = manifest_bytes(path)
    if not hmac.compare_digest(hashlib.sha256(raw).hexdigest(), expected_sha256):
        raise Refused("independent manifest digest mismatch")
    expected = validate_manifest(raw)
    actual = inventory(root)
    e = {x["key"]: (x["size"], x["sha256"]) for x in expected}
    a = {x["key"]: (x["size"], x["sha256"]) for x in actual}
    missing = len(e.keys() - a.keys())
    unexpected = len(a.keys() - e.keys())
    changed = sum(e[key] != a[key] for key in e.keys() & a.keys())
    if missing or unexpected or changed:
        raise Refused("inventory mismatch missing=%d unexpected=%d changed=%d" %
                      (missing, unexpected, changed))
    print("TOR_I11_OFFSITE_MANIFEST_VERIFIED objects=%d bytes=%d" %
          (len(actual), sum(x["size"] for x in actual)))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("snapshot", "verify"))
    parser.add_argument("--root", required=True)
    parser.add_argument("--output")
    parser.add_argument("--manifest")
    parser.add_argument("--expected-manifest-sha256")
    args = parser.parse_args()
    try:
        root = root_directory(args.root)
        if args.mode == "snapshot":
            if not args.output or args.manifest or args.expected_manifest_sha256:
                raise Refused("snapshot requires only --output")
            snapshot(root, Path(args.output))
        else:
            if not args.manifest or not args.expected_manifest_sha256 or args.output:
                raise Refused("verify requires --manifest and independent digest")
            verify(root, Path(args.manifest), args.expected_manifest_sha256)
    except (Refused, OSError) as error:
        reason = str(error) if isinstance(error, Refused) else "filesystem error"
        print("TOR_I11_MANIFEST_REFUSED: " + reason, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
