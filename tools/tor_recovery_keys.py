#!/usr/bin/env python3
"""RECOVERY-04a: verify the composition of the independently sealed TOR app keys.

Offline, no DB/S3 access. Reads plaintext only from stdin, never prints or
writes key material. Inventory pins and DB usage reports come from independent
operator records. A pass is not proof of actual decryption or DR acceptance.
"""
from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import stat
import sys

LIMIT = 65536
MASTER = "TOR_S3_ADMIN_MASTER_KEY"
V1 = "TOR_CONTENT_KEK_V1"
V2 = "TOR_CONTENT_KEK_V2"
KEY_NAMES = frozenset((MASTER, V1, V2))
IDS = {"content-kek-v1": V1, "content-kek-v2": V2}
HEX = re.compile(r"[a-f0-9]{64}\Z")


class Refused(ValueError):
    """Static error reasons only; never include input values."""


def require(condition, reason):
    if not condition:
        raise Refused(reason)


def unique_fields(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON field")
        result[key] = value
    return result


def json_object(raw, fields):
    require(0 < len(raw) <= LIMIT, "empty or oversized key evidence")
    try:
        data = json.loads(raw.decode("utf-8"), object_pairs_hook=unique_fields)
    except (UnicodeError, json.JSONDecodeError, RecursionError) as error:
        raise Refused("invalid key evidence JSON") from error
    require(isinstance(data, dict) and set(data) == fields, "unexpected key evidence fields")
    require(type(data["schema_version"]) is int and data["schema_version"] == 1,
            "unsupported key evidence schema")
    return data


def read_private(path):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid()
                and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1
                and info.st_size <= LIMIT,
                "private regular evidence file required")
        return stream.read(LIMIT + 1)


def decode_key(name, value):
    require(isinstance(value, str) and value.isascii(), "invalid key encoding")
    try:
        if name == MASTER:
            decoded = base64.b64decode(value, validate=True)
            canonical = base64.b64encode(decoded).decode("ascii")
        else:
            decoded = base64.b64decode(value + "=", altchars=b"-_", validate=True)
            canonical = base64.urlsafe_b64encode(decoded).decode("ascii").rstrip("=")
    except (ValueError, binascii.Error) as error:
        raise Refused("invalid key encoding") from error
    require(len(decoded) == 32 and canonical == value, "invalid key length or encoding")
    # Random production keys are required; this rejects the obvious zero key,
    # not every low-entropy value. No entropy claim can be proved here.
    require(any(decoded), "zero key refused")
    return decoded


def validate_active(names, active):
    require(isinstance(active, (str, type(None))), "invalid active key ID")
    if V2 in names:
        require(V1 in names, "historical V1 key required with V2")
    if V1 in names:
        require(active in IDS and IDS[active] in names, "active key missing or unknown")
    else:
        require(active is None, "active content key without content keyring")


def bundle_fingerprints(raw):
    data = json_object(raw, {"schema_version", "active_kek_id", "keys"})
    keys = data["keys"]
    require(isinstance(keys, dict) and 0 < len(keys) <= len(KEY_NAMES)
            and set(keys) <= KEY_NAMES, "unknown or missing application key material")
    validate_active(keys, data["active_kek_id"])
    decoded = {name: decode_key(name, value) for name, value in keys.items()}
    require(len(set(decoded.values())) == len(decoded), "application key reuse refused")
    return {"schema_version": 1, "active_kek_id": data["active_kek_id"],
            "key_sha256": {name: hashlib.sha256(key).hexdigest()
                           for name, key in decoded.items()}}


def db_required_keys(raw):
    data = json_object(raw, {"schema_version", "storage_kek_ids", "admin_s3_profile_count"})
    ids = data["storage_kek_ids"]
    require(isinstance(ids, list) and len(ids) <= len(IDS)
            and all(isinstance(key, str) and key in IDS for key in ids)
            and len(set(ids)) == len(ids), "unknown or duplicate database KEK ID")
    count = data["admin_s3_profile_count"]
    require(type(count) is int and count in (0, 1), "invalid S3 profile count")
    required = {IDS[key] for key in ids}
    if required:
        required.add(V1)  # Rust keyring still requires V1 even for V2-only rows.
    if count:
        required.add(MASTER)
    return required


def validated_inventory(raw):
    inv = json_object(raw, {"schema_version", "active_kek_id", "key_sha256"})
    keys = inv["key_sha256"]
    require(isinstance(keys, dict) and 0 < len(keys) <= len(KEY_NAMES)
            and set(keys) <= KEY_NAMES, "invalid pinned key inventory")
    require(all(isinstance(digest, str) and HEX.fullmatch(digest)
                and digest != "0" * 64 for digest in keys.values()),
            "invalid pinned key fingerprint")
    require(len(set(keys.values())) == len(keys), "pinned key reuse refused")
    validate_active(keys, inv["active_kek_id"])
    return inv


def coverage(inventory, usage_raw):
    required = db_required_keys(usage_raw)
    require(required <= set(inventory["key_sha256"]),
            "database references missing historical key material")


def capture(bundle_raw, usage_raw, output):
    inventory = bundle_fingerprints(bundle_raw)
    coverage(inventory, usage_raw)
    raw = (json.dumps(inventory, sort_keys=True, separators=(",", ":")) + "\n").encode()
    # No overwrite or symlink following. Only fingerprints are written.
    fd = os.open(output, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())
    return len(inventory["key_sha256"])


def verify(bundle_raw, usage_raw, inventory_raw, expected_sha):
    require(isinstance(expected_sha, str) and HEX.fullmatch(expected_sha)
            and expected_sha != "0" * 64, "independent inventory SHA-256 required")
    require(hmac.compare_digest(hashlib.sha256(inventory_raw).hexdigest(), expected_sha),
            "independent key inventory digest mismatch")
    expected = validated_inventory(inventory_raw)
    actual = bundle_fingerprints(bundle_raw)
    coverage(expected, usage_raw)
    require(expected["active_kek_id"] == actual["active_kek_id"],
            "active KEK differs from approved archive")
    require(set(expected["key_sha256"]) == set(actual["key_sha256"]),
            "missing or unexpected application key")
    for name, digest in expected["key_sha256"].items():
        require(hmac.compare_digest(digest, actual["key_sha256"][name]),
                "application key fingerprint mismatch")
    return len(actual["key_sha256"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest="mode", required=True)
    create = modes.add_parser("capture", help="pre-incident inventory from trusted keys")
    create.add_argument("--db-usage", type=Path, required=True)
    create.add_argument("--output", type=Path, required=True)
    check = modes.add_parser("verify", help="validate recovered keys against independent pin")
    check.add_argument("--db-usage", type=Path, required=True)
    check.add_argument("--inventory", type=Path, required=True)
    check.add_argument("--expected-inventory-sha256", required=True)
    args = parser.parse_args()
    try:
        require(not sys.stdin.isatty(), "pipe a private key bundle; never type keys")
        raw = sys.stdin.buffer.read(LIMIT + 1)
        usage = read_private(args.db_usage)
        if args.mode == "capture":
            count = capture(raw, usage, args.output)
            status = "INVENTORY_CREATED"
        else:
            count = verify(raw, usage, read_private(args.inventory),
                           args.expected_inventory_sha256)
            status = "VERIFIED"
    except (Refused, OSError, ValueError, OverflowError, RecursionError) as error:
        reason = str(error) if isinstance(error, Refused) else "local key verification failed"
        print("TOR_RECOVERY_KEYS_REFUSED: " + reason, file=sys.stderr)
        return 2
    print(f"TOR_RECOVERY_KEYS_{status} keys={count} "
          "decryption_tested=false recovery_accepted=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
