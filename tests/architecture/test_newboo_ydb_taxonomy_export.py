"""Read-only YDB snapshot export tests without a real connection."""
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import newboo_ydb_taxonomy_export as export

ID = "11111111-1111-4111-8111-111111111111"

class Result:
    def __init__(self, rows):
        self.rows = rows
    def __enter__(self):
        return [types.SimpleNamespace(
            rows=[types.SimpleNamespace(**row) for row in self.rows]
        )]
    def __exit__(self, *args):
        return False

class Tx:
    def __init__(self, broken=False):
        self.calls = []
        self.broken = broken
    def execute(self, sql, commit_tx=False):
        self.calls.append((sql, commit_tx))
        if "COUNT(*)" in sql:
            return Result([{"total_rows": 2 if self.broken and "book_taxonomies" in sql else 1}])
        if "ORDER BY taxonomy_id" in sql:
            return Result([{"taxonomy_id": ID, "taxonomy_type": "tag",
                            "title": "Система", "status": "active"}])
        if "ORDER BY alias_id" in sql:
            return Result([{"alias_id": "a1", "taxonomy_id": ID,
                            "taxonomy_type": "tag", "alias": "System"}])
        raise AssertionError("unexpected YDB query")

class Session:
    def __init__(self, tx):
        self.tx = tx
        self.modes = []
    def transaction(self, mode):
        self.modes.append(mode)
        return self.tx

class SDK:
    @staticmethod
    def QuerySnapshotReadOnly():
        return "snapshot-readonly"

class YdbExportTests(unittest.TestCase):
    def test_complete_consistent_snapshot(self):
        tx = Tx()
        session = Session(tx)
        obj = export.collect(session, SDK, "")
        self.assertEqual(session.modes, ["snapshot-readonly"])
        self.assertEqual(obj["expected_counts"], {"taxonomies": 1, "aliases": 1})
        self.assertEqual([v for _, v in tx.calls], [False, False, False, True])
        for sql, _ in tx.calls:
            self.assertIn("SELECT", sql)
            for forbidden in ("LIMIT", "INSERT", "UPDATE", "DELETE", "DROP"):
                self.assertNotIn(forbidden, sql.upper())

    def test_count_mismatch_is_fatal(self):
        with self.assertRaisesRegex(ValueError, "differs"):
            export.collect(Session(Tx(broken=True)), SDK, "")

    def test_sql_identifiers_are_fixed(self):
        self.assertEqual(export.ident("prefix", "book_taxonomies"),
                         chr(96) + "prefix/book_taxonomies" + chr(96))
        for prefix in ("../bad", "evil;DROP", "abc/" + chr(96)):
            with self.subTest(prefix=prefix):
                with self.assertRaises(ValueError):
                    export.ident(prefix, "book_taxonomies")
        with self.assertRaises(ValueError):
            export.ident("", "users")

    def test_target_requires_exact_database_confirmation(self):
        export.validate_target("grpcs://localhost:2135", "/local", "/local")
        with self.assertRaisesRegex(ValueError, "confirm"):
            export.validate_target("grpcs://localhost:2135", "/local", "/other")

    def test_json_atomic_and_existing_file_protected(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "snapshot.json"
            data = export.collect(Session(Tx()), SDK, "")
            export.save(str(path), data)
            self.assertEqual(json.loads(path.read_text(encoding="utf-8"))["taxonomies"][0]["title"], "Система")
            with self.assertRaisesRegex(ValueError, "overwritten"):
                export.save(str(path), data)

    def test_bad_confirmation_never_imports_sdk(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "data.json"
            self.assertEqual(export.main([
                "--endpoint", "grpcs://localhost:2135", "--database", "/local",
                "--confirm-database", "/wrong", "--out", str(out),
            ]), 2)
            self.assertFalse(out.exists())

if __name__ == "__main__":
    unittest.main()
