import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
from d001_single_space_preview import preview
from i11_object_manifest import Refused


class D001SingleSpacePreview(unittest.TestCase):
    def test_exact_matching_snapshot_bytes_and_exclusions(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            primary = root / "primary.json"
            backup = root / "backup.json"
            keys = [f"private/free-chapters/1/{i}.md" for i in range(1, 7)]
            digest = "ab" * 32
            rows = [{"key": key, "size": i * 10, "sha256": digest}
                    for i, key in enumerate(keys, 1)]

            def write(path, entries):
                raw = json.dumps({
                    "schema_version": 1,
                    "created_utc": "2026-11-17T00:00:00Z",
                    "objects": sorted(entries, key=lambda row: row["key"]),
                }).encode()
                path.write_bytes(raw)
                return hashlib.sha256(raw).hexdigest()

            # 1 matches, 2 missing primary, 3 changed primary,
            # 4 missing backup, 5 changed backup, 6 matches.
            pd = write(primary, [
                rows[0], dict(rows[2], size=31), rows[3], rows[4], rows[5],
            ])
            bd = write(backup, [
                rows[0], rows[1], rows[2], dict(rows[4], sha256="cd" * 32), rows[5],
            ])
            lines = [f"{row['key']},{row['size']},{row['sha256']}\n" for row in rows]

            def check(report="".join(lines), primary_digest=pd, backup_digest=bd):
                return preview(io.StringIO(report), primary, primary_digest,
                               backup, backup_digest)

            result = check()
            self.assertEqual(result["mode"], "snapshot_preview_only")
            self.assertIs(result["deletion_authorized"], False)
            self.assertEqual(result["candidates"], {"objects": 6, "registered_bytes": 210})
            self.assertEqual(result["dispositions"], {
                "matched": {"objects": 2, "registered_bytes": 70},
                "primary_missing": {"objects": 1, "registered_bytes": 20},
                "primary_changed": {"objects": 1, "registered_bytes": 30},
                "backup_missing": {"objects": 1, "registered_bytes": 40},
                "backup_changed": {"objects": 1, "registered_bytes": 50},
            })
            self.assertNotIn("private/free-chapters", json.dumps(result))
            self.assertEqual(check("")["candidates"], {"objects": 0, "registered_bytes": 0})
            with self.assertRaisesRegex(Refused, "digest mismatch"):
                check(primary_digest="00" * 32)
            with self.assertRaisesRegex(Refused, "digest mismatch"):
                check(backup_digest="00" * 32)
            with self.assertRaisesRegex(Refused, "duplicate or unsorted"):
                check(lines[0] * 2)
            with self.assertRaisesRegex(Refused, "invalid FREE SINGLE"):
                check(lines[0].replace(".md", ".zst"))
            with self.assertRaisesRegex(Refused, "invalid FREE SINGLE"):
                check(lines[0].replace(",10,", ",200001,"))
