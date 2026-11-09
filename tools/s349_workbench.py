"""Temporary source-only validation preparation; removed before acceptance."""
from pathlib import Path
import sys
sys.dont_write_bytecode = True
root = Path(__file__).resolve().parents[1]
p = root / 'crates/storage/tests/free_chapter_storage_pg.rs'
s = p.read_text()
if '#![cfg(feature = "pg")]' not in s:
    s = s.replace('use sqlx::', '#![cfg(feature = "pg")]\nuse sqlx::', 1)
    p.write_text(s)
# Export public PostgreSQL18/MinIO/Playwright tools with dereferenced PG share files.
# Workflow edits use the separately authorized repository connector.
