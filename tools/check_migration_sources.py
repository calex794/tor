#!/usr/bin/env python3
"""Fail closed if any of the nine pinned donor SQL migration blobs changes."""
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
MIGRATIONS = ROOT / "migrations"
MANIFEST = MIGRATIONS / "SOURCES.tsv"
entries = []
for line in MANIFEST.read_text(encoding="utf-8").splitlines():
    if not line or line.startswith("#"):
        continue
    parts = line.split("\t")
    if len(parts) != 2 or not re.fullmatch(r"[0-9a-f]{40}", parts[0]):
        raise SystemExit("Invalid migration manifest row")
    if not re.fullmatch(r"\d{12}_[a-z0-9_]+\.sql", parts[1]):
        raise SystemExit("Invalid migration filename")
    entries.append((parts[0], parts[1]))
names = [name for _, name in entries]
if len(names) != 9 or sorted(names) != sorted(p.name for p in MIGRATIONS.glob("*.sql")):
    raise SystemExit("Pinned source list does not match exactly nine SQL migrations")
if len(set(name[:12] for name in names)) != 9 or names != sorted(names):
    raise SystemExit("Migrations must have unique, monotonically ordered SQLx versions")
for expected, name in entries:
    actual = subprocess.check_output(
        ["git", "hash-object", str(MIGRATIONS / name)],
        text=True,
        cwd=ROOT,
    ).strip()
    if actual != expected:
        raise SystemExit(f"Migration {name} differs from the pinned donor blob")
print("Nine original bokarust SQL migrations unchanged and ordered")
