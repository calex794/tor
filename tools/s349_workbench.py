"""Temporary task-only deterministic patch importer; removed before acceptance."""
from pathlib import Path
import re
import sys

root = Path(__file__).resolve().parents[1]
p = root / 'crates/storage/tests/free_chapter_storage_pg.rs'
s = p.read_text()
if '#![cfg(feature = "pg")]' not in s:
    s = s.replace('use sqlx::', '#![cfg(feature = "pg")]\nuse sqlx::', 1)
    p.write_text(s)
sys.path.insert(0, str(root / 'tools'))
from ci_test_map import SUITES, SUITE_JOB_IDS, workflow_gate
p = root / '.github/workflows/tor-integration.yml'
s = p.read_text()
for suite in ('postgres', 'storage_s3'):
    filename, _ = SUITES[suite]
    manual = (root / '.github/workflows' / filename).read_text()
    job = SUITE_JOB_IDS[suite]
    pattern = r'^  ' + re.escape(job) + r':\n.*?(?=^  [A-Za-z0-9_-]+:\n|\Z)'
    body = re.search(pattern, manual, re.M | re.S).group()
    gate = workflow_gate(suite)
    body = body.replace('    needs: select\n    if: ' + gate + '\n', '    needs: [select, preflight]\n    if: ' + gate.replace('!cancelled() && ', "!cancelled() && needs.preflight.result == 'success' && ", 1) + '\n', 1)
    s, n = re.subn(pattern, lambda m: body, s, count=1, flags=re.M | re.S)
    assert n == 1, job
p.write_text(s)
