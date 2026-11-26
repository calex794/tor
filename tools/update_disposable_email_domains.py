#!/usr/bin/env python3
"""Prepare a reviewed, exact CC0 upstream snapshot. No network calls."""
import argparse
import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "crates/accounts/data"
BLOCKLIST = DATA / "disposable_email_domains.txt"
METADATA = DATA / "disposable_email_source.json"
REPOSITORY = "disposable-email-domains/disposable-email-domains"
SOURCE_PATH = "disposable_email_blocklist.conf"
COMMON = (
    "gmail.com", "googlemail.com", "yandex.ru", "mail.ru",
    "outlook.com", "hotmail.com", "proton.me", "icloud.com",
)
HEX = re.compile(r"[0-9a-f]{40}")
LABEL = re.compile(r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?")


def git_blob(data):
    return hashlib.sha1(b"blob " + str(len(data)).encode("ascii") + b"\0" + data).hexdigest()


def validate(data):
    if len(data) > 8 * 1024 * 1024 or not data.endswith(b"\n"):
        raise ValueError("unexpected snapshot size or missing final newline")
    domains = data.decode("ascii").split("\n")[:-1]
    if not 1000 <= len(domains) <= 50000:
        raise ValueError("unexpected domain count")
    if domains != sorted(set(domains)):
        raise ValueError("domains must be unique and sorted")
    for domain in domains:
        labels = domain.split(".")
        if (
            not 1 <= len(domain) <= 252
            or len(labels) < 2
            or labels[-1].isdigit()
            or any(len(label) > 63 or not LABEL.fullmatch(label) for label in labels)
        ):
            raise ValueError(f"invalid domain: {domain!r}")
    blocked = set(domains)
    for domain in COMMON:
        labels = domain.split(".")
        if any(".".join(labels[i:]) in blocked for i in range(len(labels) - 1)):
            raise ValueError(f"protected common provider matched: {domain}")
    return domains


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshot", type=Path, help="downloaded upstream file")
    parser.add_argument("--commit", required=True, help="reviewed upstream commit SHA")
    parser.add_argument("--blob", required=True, help="upstream Git blob SHA")
    parser.add_argument("--write", action="store_true", help="write candidate for PR review")
    args = parser.parse_args()
    if not HEX.fullmatch(args.commit) or not HEX.fullmatch(args.blob):
        parser.error("commit and blob must be full lowercase Git SHA-1 values")
    if args.snapshot.stat().st_size > 8 * 1024 * 1024:
        parser.error("snapshot exceeds size limit")
    data = args.snapshot.read_bytes()
    if git_blob(data) != args.blob:
        parser.error("Git blob SHA mismatch")
    domains = validate(data)
    old = set(BLOCKLIST.read_text(encoding="ascii").splitlines())
    new = set(domains)
    print(f"{len(old)} -> {len(new)} domains; +{len(new - old)} -{len(old - new)}")
    if not args.write:
        return
    metadata = {
        "repository": REPOSITORY,
        "path": SOURCE_PATH,
        "commit": args.commit,
        "git_blob_sha1": args.blob,
        "license": "CC0-1.0",
        "domains": len(domains),
    }
    BLOCKLIST.write_bytes(data)
    METADATA.write_text(json.dumps(metadata, indent=2) + "\n", encoding="ascii")
    print("Candidate written. Review the diff and run CI before accepting it.")


if __name__ == "__main__":
    main()
