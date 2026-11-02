#!/usr/bin/env python3
"""Fail closed on conflicting SQLx versions in the actual checkout tree.

GitHub pull_request checks out the merged base+head tree. A PR can have a
green CI on its stale branch while its new SQLx version already exists in main.
This check is local-only: no database, network, credentials or DDL.
"""
import argparse
import hashlib
from pathlib import Path
import re
import sys

MIGRATION = re.compile(r"^(?P<version>[0-9]{12})_[a-z0-9_]+\.sql$")


class MigrationTreeError(ValueError):
    """The proposed migration tree is incomplete or internally inconsistent."""


def check_migration_tree(directory: Path) -> int:
    if not directory.is_dir() or directory.is_symlink():
        raise MigrationTreeError("SQLx migration directory unavailable")
    files = sorted(directory.glob("*.sql"))
    if not files:
        raise MigrationTreeError("SQLx migration directory is empty")

    versions = {}
    contents = {}
    for path in files:
        match = MIGRATION.fullmatch(path.name)
        if match is None or path.is_symlink() or not path.is_file():
            raise MigrationTreeError(f"Invalid SQLx migration file: {path.name}")
        version = match.group("version")
        try:
            data = path.read_bytes()
            data.decode("utf-8")
        except (OSError, UnicodeError) as exc:
            raise MigrationTreeError(f"Unreadable SQLx migration: {path.name}") from exc
        if not data:
            raise MigrationTreeError(f"Empty SQLx migration: {path.name}")
        if version in versions:
            raise MigrationTreeError(
                f"Duplicate SQLx version {version}: "
                f"{versions[version]} and {path.name}"
            )
        digest = hashlib.sha384(data).digest()
        if digest in contents:
            other_version, other_filename = contents[digest]
            raise MigrationTreeError(
                "Identical SQLx content under different versions "
                f"{other_version}/{version}: {other_filename} and {path.name}"
            )
        versions[version] = path.name
        contents[digest] = (version, path.name)
    return len(files)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", nargs="?", type=Path, default=Path("migrations"))
    args = parser.parse_args(argv)
    try:
        count = check_migration_tree(args.directory)
    except MigrationTreeError as exc:
        print(f"SQLx checkout tree BLOCK: {exc}", file=sys.stderr)
        return 2
    print(f"SQLx checkout tree OK: {count} unique versions and SQL payloads")
    return 0


if __name__ == "__main__":
    sys.exit(main())
