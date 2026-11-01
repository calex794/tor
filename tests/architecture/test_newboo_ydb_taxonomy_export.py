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
    def __init__(self, indexed_rows):
        self.indexed_rows = indexed_rows
    def __enter__(self):
        return [
            types.SimpleNamespace(
                index=index,
                truncated=truncated,
                rows=[types.SimpleNamespace(**row) for row in rows],
            )
            for index, rows, truncated in self.indexed_rows
        ]
    def __exit__(self, *args):
        return False

class Tx:
    def __init__(self, broken=False, omitted=None, truncated=None):
        self.calls = []
        self.broken = broken
        self.omitted = omitted
        self.truncated = truncated
    def execute(self, sql, commit_tx=False, concurrent_result_sets=None):
        self.calls.append((sql, commit_tx, concurrent_result_sets))
        if sql.count("SELECT ") != 4:
            raise AssertionError("snapshot must use exactly four SELECT statements")
        groups = [
            (0, [{"total_rows": 2 if self.broken else 1}]),
            (1, [{"total_rows": 1}]),
            (2, [{"taxonomy_id": ID, "taxonomy_type": "tag",
                  "title": "Система", "status": "active"}]),
            (3, [{"alias_id": "a1", "taxonomy_id": ID,
                  "taxonomy_type": "tag", "alias": "System"}]),
        ]
        return Result([
            (index, rows, index == self.truncated)
            for index, rows in groups if index != self.omitted
        ])

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
        self.assertEqual(len(tx.calls), 1)
        sql, commit, concurrent = tx.calls[0]
        self.assertTrue(commit)
        self.assertFalse(concurrent)
        self.assertEqual(sql.count("SELECT "), 4)
        for forbidden in ("LIMIT", "INSERT", "UPDATE", "DELETE", "DROP"):
            self.assertNotIn(forbidden, sql.upper())

    def test_count_mismatch_is_fatal(self):
        with self.assertRaisesRegex(ValueError, "differs"):
            export.collect(Session(Tx(broken=True)), SDK, "")

    def test_missing_result_set_or_truncation_is_fatal(self):
        with self.assertRaisesRegex(ValueError, "omitted"):
            export.collect(Session(Tx(omitted=3)), SDK, "")
        with self.assertRaisesRegex(ValueError, "truncated"):
            export.collect(Session(Tx(truncated=2)), SDK, "")

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
