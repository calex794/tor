#!/usr/bin/env python3
"""RECOVERY-04b: restore exactly an approved object snapshot from offsite S3.

Operator-only. Default validates the plan without network or writes.
Never contacts the primary bucket, writes remote objects, or starts TOR.
"""
from __future__ import annotations

import argparse
import configparser
import hashlib
import hmac
import io
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys

from i11_object_manifest import (
    HEX_SHA256, MAX_MANIFEST_BYTES, Refused, inventory, validate_manifest,
)
from tor_recovery_host import (
    NAME, PARENT, Refused as HostRefused, reject_live_markers, validate_target,
)

ROOT = PARENT / NAME
CONFIG_LIMIT = 65536
REMOTE = "tor-offsite-read:objects"
RCLONE = "/usr/bin/rclone"
SAFE_OBJECT = re.compile(r"[A-Za-z0-9._/-]+\Z")
BUCKET = re.compile(r"[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]\Z")
ENDPOINT = re.compile(r"https://[a-zA-Z0-9](?:[a-zA-Z0-9.-]*[a-zA-Z0-9])?\Z")
CRYPT_ALLOWED = {
    "type", "remote", "password", "password2", "filename_encryption",
    "directory_name_encryption", "filename_encoding",
}
S3_ALLOWED = {
    "type", "provider", "env_auth", "access_key_id", "secret_access_key",
    "session_token", "region", "endpoint", "force_path_style", "no_check_bucket",
}


def require(ok, message):
    if not ok:
        raise Refused(message)


def verify_pin(raw, expected):
    require(isinstance(expected, str) and HEX_SHA256.fullmatch(expected)
            and expected != "0" * 64, "independent SHA-256 pin required")
    require(hmac.compare_digest(hashlib.sha256(raw).hexdigest(), expected),
            "independent file digest mismatch")


def private_file(path, limit):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid()
                and stat.S_IMODE(info.st_mode) == 0o600
                and info.st_nlink == 1 and 0 < info.st_size <= limit,
                "unsafe private evidence file")
        raw = stream.read(limit + 1)
    require(0 < len(raw) <= limit, "oversized or empty evidence file")
    return raw


def approved_objects(raw, pin):
    verify_pin(raw, pin)
    entries = validate_manifest(raw)
    keys = {entry["key"] for entry in entries}
    for key in keys:
        require(SAFE_OBJECT.fullmatch(key) is not None,
                "object name outside supported restore grammar")
        parts = key.split("/")
        require(not any("/".join(parts[:index]) in keys
                        for index in range(1, len(parts))),
                "object path conflicts with another object directory")
    return entries


def validate_config(raw, pin, endpoint, archive_root):
    verify_pin(raw, pin)
    require(ENDPOINT.fullmatch(endpoint) is not None and ".." not in endpoint,
            "explicit approved HTTPS S3 endpoint required")
    bucket, sep, prefix = archive_root.partition("/")
    require(sep and BUCKET.fullmatch(bucket) and ".." not in bucket
            and bucket != "tore" and SAFE_OBJECT.fullmatch(prefix)
            and all(part not in ("", ".", "..") for part in prefix.split("/")),
            "independent backup bucket and literal archive prefix required")
    cfg = configparser.ConfigParser(interpolation=None, strict=True)
    try:
        cfg.read_string(raw.decode("ascii"))
    except (configparser.Error, UnicodeError) as error:
        raise Refused("invalid private restore configuration") from error
    require(not cfg.defaults() and set(cfg.sections()) == {"tor-offsite-read", "tor-offsite-s3"},
            "only dedicated crypt and S3 restore aliases permitted")
    crypt, s3 = cfg["tor-offsite-read"], cfg["tor-offsite-s3"]
    require(set(crypt) <= CRYPT_ALLOWED and set(s3) <= S3_ALLOWED,
            "unexpected restore backend option")
    for section in (crypt, s3):
        require(all(value and value.isascii()
                    and not any(ch.isspace() or ord(ch) < 32 for ch in value)
                    for value in section.values()), "invalid restore configuration value")
    require(crypt.get("type") == "crypt"
            and crypt.get("remote") == "tor-offsite-s3:" + archive_root
            and bool(crypt.get("password"))
            and crypt.get("filename_encryption", "standard") == "standard"
            and crypt.get("directory_name_encryption", "true") in ("true", "false")
            and crypt.get("filename_encoding", "base32") in ("base32", "base64", "base32768"),
            "crypt backend differs from approved archive")
    require(s3.get("type") == "s3" and s3.get("endpoint") == endpoint
            and s3.get("env_auth") == "false"
            and all(s3.get(name) for name in
                    ("provider", "region", "access_key_id", "secret_access_key")),
            "dedicated explicit S3 restore credentials and endpoint required")
    for name in ("force_path_style", "no_check_bucket"):
        require(s3.get(name, "true") in ("true", "false"),
                "invalid S3 boolean option")
    # IAM/retention are not proved by names, pins or a read-only command.
    # They require independent operator/provider acceptance.


def check_dir(path, mode, empty=False):
    info = path.lstat()
    require(stat.S_ISDIR(info.st_mode) and path.resolve(strict=True) == path
            and info.st_uid == os.geteuid() and stat.S_IMODE(info.st_mode) == mode,
            "unsafe recovery workspace directory")
    if empty:
        require(not any(path.iterdir()), "restore destination is not empty")


def create_private(path, raw):
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())


def command(root):
    evidence = root / "evidence"
    return [
        RCLONE, "--config", str(evidence / "objects-rclone.conf"),
        "--cache-dir", str(evidence / "objects-cache"),
        "copy", REMOTE, str(root / "objects"),
        "--files-from-raw", str(evidence / "objects-selection.txt"),
        "--no-traverse", "--immutable", "--transfers", "2", "--checkers", "4",
        "--retries", "1", "--low-level-retries", "3", "--log-level", "ERROR",
    ]


def verify_staged(root, entries):
    sizes = {entry["key"]: entry["size"] for entry in entries}
    actual = inventory(root, expected_sizes=sizes)
    require(actual == entries, "restored object set or digest differs from approved manifest")
    return len(entries)


def prepare_plan(args, root):
    check_dir(root, 0o710)
    check_dir(root / "objects", 0o700, empty=True)
    evidence = root / "evidence"
    check_dir(evidence, 0o700)
    for name in ("objects-attempt", "objects-selection.txt", "objects-cache"):
        require(not os.path.lexists(evidence / name),
                "previous object restore attempt exists; inspect separately")
    entries = approved_objects(private_file(evidence / "objects-manifest.json",
                                             MAX_MANIFEST_BYTES),
                               args.expected_manifest_sha256)
    cfg = private_file(evidence / "objects-rclone.conf", CONFIG_LIMIT)
    validate_config(cfg, args.expected_config_sha256, args.endpoint, args.archive_root)
    total = sum(entry["size"] for entry in entries)
    # The operator also needs disk quota/headroom for corrupted or oversized
    # remote objects; this is only a lower bound from the retained inventory.
    require(shutil.disk_usage(root / "objects").free >= total,
            "insufficient space for selected snapshot")
    return entries, total


def restore(args, root, entries):
    require(args.operator_confirm_read_only_repository,
            "independent read-only IAM review confirmation required")
    require(os.access(RCLONE, os.X_OK), "trusted rclone installation required")
    evidence = root / "evidence"
    # Permanent attempt marker, no overwrite/retry into partially copied data.
    create_private(evidence / "objects-attempt", b"started\n")
    create_private(evidence / "objects-selection.txt",
                   "".join(entry["key"] + "\n" for entry in entries).encode("ascii"))
    (evidence / "objects-cache").mkdir(mode=0o700)
    # No inherited AWS/RCLONE variables or home configuration.
    # The private, pinned rclone config must not be changed during recovery.
    result = subprocess.run(
        command(root), check=False, stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL, cwd=root,
        env={"PATH": "/usr/bin:/bin", "HOME": str(evidence), "LC_ALL": "C"},
    )
    require(result.returncode == 0, "offsite download failed; partial workspace retained")
    verify_pin(private_file(evidence / "objects-rclone.conf", CONFIG_LIMIT),
               args.expected_config_sha256)
    check_dir(root / "objects", 0o700)
    verify_staged(root / "objects", entries)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-machine-id", required=True)
    parser.add_argument("--target-machine-id", required=True)
    parser.add_argument("--expected-manifest-sha256", required=True)
    parser.add_argument("--expected-config-sha256", required=True)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--archive-root", required=True)
    parser.add_argument("--restore", action="store_true")
    parser.add_argument("--operator-confirm-read-only-repository", action="store_true")
    args = parser.parse_args()
    try:
        require(os.geteuid() == 0 and "GITHUB_ACTIONS" not in os.environ,
                "operator-only independent recovery host required; CI prohibited")
        machine = Path("/etc/machine-id").read_text(encoding="ascii").strip()
        validate_target(args.source_machine_id, args.target_machine_id, machine, 15432)
        reject_live_markers()
        os.umask(0o077)
        entries, total = prepare_plan(args, ROOT)
        if args.restore:
            restore(args, ROOT, entries)
    except (Refused, HostRefused, OSError, ValueError, OverflowError,
            RecursionError, subprocess.SubprocessError) as error:
        reason = str(error) if isinstance(error, (Refused, HostRefused)) else "local restore failed"
        print("TOR_RECOVERY_OBJECTS_REFUSED: " + reason, file=sys.stderr)
        return 2
    status = "VERIFIED" if args.restore else "PLAN_ONLY"
    print(f"TOR_RECOVERY_OBJECTS_{status} objects={len(entries)} bytes={total} "
          "db_receipts_verified=false decryption_tested=false recovery_accepted=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
