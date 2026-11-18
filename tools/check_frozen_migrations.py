#!/usr/bin/env python3
"""Reject changes to SQLx migrations already present in the base Git commit.

A clean PostgreSQL install alone cannot protect a retained database: SQLx
records the applied migration version and checksum. Once a migration is merged,
changing its contents, name or version would make future upgrades unsafe.

New migrations may be added. Work-in-progress migrations that have not reached
the base branch may still be renumbered to resolve a PR conflict.
"""

import argparse
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
MIGRATION = re.compile(r"^migrations/(?P<version>[0-9]{12})_[a-z0-9_]+\.sql$")


class MigrationHistoryError(ValueError):
    """A retained database could no longer replay the published migrations."""


def migration_tree(repo: Path, ref: str) -> dict[str, str]:
    result = subprocess.run(
        ["git", "-C", str(repo), "ls-tree", "-r", "-z", ref, "--", "migrations"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode:
        raise MigrationHistoryError(
            f"Cannot inspect Git revision {ref!r}: "
            + result.stderr.decode("utf-8", errors="replace").strip()
        )

    files: dict[str, str] = {}
    seen_versions: dict[str, str] = {}
    for entry in result.stdout.split(b"\0"):
        if not entry:
            continue
        metadata, separator, path_bytes = entry.partition(b"\t")
        if not separator:
            raise MigrationHistoryError("Malformed Git tree entry")
        fields = metadata.split()
        if len(fields) != 3 or fields[1] != b"blob":
            raise MigrationHistoryError("Invalid Git migration tree entry")
        try:
            path = path_bytes.decode("utf-8")
            mode, sha = fields[0].decode("ascii"), fields[2].decode("ascii")
        except UnicodeError as exc:
            raise MigrationHistoryError("Invalid Git migration path or blob ID") from exc
        if not path.endswith(".sql"):
            continue
        match = MIGRATION.fullmatch(path)
        if match is None or mode != "100644" or not re.fullmatch(r"[0-9a-f]{40,64}", sha):
            raise MigrationHistoryError(f"Unsafe SQLx migration in {ref}: {path}")
        version = match.group("version")
        if version in seen_versions:
            raise MigrationHistoryError(
                f"Duplicate SQLx version {version}: {seen_versions[version]} and {path}"
            )
        seen_versions[version] = path
        files[path] = sha
    if not files:
        raise MigrationHistoryError(f"No SQLx migrations at Git revision {ref!r}")
    return files


def check_frozen_migrations(repo: Path, base_ref: str, head_ref: str = "HEAD") -> tuple[int, int]:
    """Return the frozen and newly added counts, or fail on history mutation."""
    base = migration_tree(repo, base_ref)
    head = migration_tree(repo, head_ref)
    for path, old_sha in base.items():
        new_sha = head.get(path)
        if new_sha is None:
            raise MigrationHistoryError(f"Previously merged SQLx migration removed or renamed: {path}")
        if new_sha != old_sha:
            raise MigrationHistoryError(f"Previously merged SQLx migration modified: {path}")
    return len(base), len(head) - len(base)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-ref", required=True, help="Git SHA of PR base or previous main")
    parser.add_argument("--head-ref", default="HEAD", help="Git revision to verify")
    parser.add_argument("--repo", type=Path, default=ROOT, help="Git checkout")
    args = parser.parse_args(argv)
    try:
        frozen, added = check_frozen_migrations(args.repo, args.base_ref, args.head_ref)
    except MigrationHistoryError as exc:
        print(f"FND-01 migration history BLOCK: {exc}", file=sys.stderr)
        return 2
    print(f"FND-01 migration history OK: {frozen} frozen SQLx files, {added} additive files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
