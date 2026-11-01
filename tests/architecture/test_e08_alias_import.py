"""Offline regressions for explicitly reviewed E-08 legacy Book aliases."""
import csv
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import e08_prepare_alias_import as aliases  # noqa: E402


class ReviewedAliasesTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.source = Path(self.temp.name) / "newboo.csv"
        self.mapping = Path(self.temp.name) / "reviewed.csv"

    def csv(self, path, fields, rows):
        with path.open("w", encoding="utf-8", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=fields)
            writer.writeheader()
            writer.writerows(rows)

    def given(self, source_rows=None, map_rows=None):
        if source_rows is None:
            source_rows = [
                {"book_id": "10", "public_id": "Book-Q", "slug": "Launch-Day"},
                {"book_id": "11", "public_id": "12345", "slug": "Another-Book"},
            ]
        if map_rows is None:
            map_rows = [
                {"newboo_book_id": "10", "tor_book_id": "42", "tor_source_identity": "en:book'10", "tor_target_language": "ru", "reviewed": "yes"},
                {"newboo_book_id": "11", "tor_book_id": "43", "tor_source_identity": "source:11", "tor_target_language": "ru", "reviewed": "yes"},
            ]
        self.csv(self.source, aliases.SOURCE_FIELDS, source_rows)
        self.csv(self.mapping, aliases.MAP_FIELDS, map_rows)

    def test_verified_rows_generate_canonical_transactional_sql(self):
        self.given()
        entries, unsupported = aliases.prepare(self.source, self.mapping)
        self.assertEqual([entry[0] for entry in entries], ["another-book", "book-q", "launch-day"])
        self.assertEqual(unsupported, [(11, "public_id", "12345")])
        sql = aliases.render_sql(entries)
        self.assertIn("('book-q',42,'en:book''10','ru')", sql)
        self.assertIn("LEFT JOIN books b ON b.id=a.book_id", sql)
        self.assertIn("b.source_identity=a.source_identity", sql)
        self.assertIn("ON CONFLICT (alias_key) DO NOTHING;", sql)
        self.assertIn("LOCK TABLE book_legacy_mail_refs", sql)
        self.assertTrue(sql.endswith("COMMIT;\n"))
        self.assertNotIn("'12345'", sql)
        self.assertEqual(sql, aliases.render_sql(entries))

    def test_duplicate_source_alias_is_rejected_even_if_other_source_is_unmapped(self):
        self.given(source_rows=[
            {"book_id": "10", "public_id": "SAME", "slug": "first"},
            {"book_id": "11", "public_id": "same", "slug": "second"},
        ], map_rows=[
            {"newboo_book_id": "10", "tor_book_id": "42", "tor_source_identity": "book:10", "tor_target_language": "ru", "reviewed": "yes"},
        ])
        with self.assertRaisesRegex(aliases.ImportErrorDetail, "multiple source books"):
            aliases.prepare(self.source, self.mapping)

    def test_review_and_source_identity_are_mandatory(self):
        self.given()
        rows = [
            {"newboo_book_id": "10", "tor_book_id": "42", "tor_source_identity": "source:10", "tor_target_language": "ru", "reviewed": "no"},
        ]
        self.csv(self.mapping, aliases.MAP_FIELDS, rows)
        with self.assertRaisesRegex(aliases.ImportErrorDetail, "reviewed=yes"):
            aliases.prepare(self.source, self.mapping)
        rows[0]["reviewed"] = "yes"
        rows[0]["tor_source_identity"] = ""
        self.csv(self.mapping, aliases.MAP_FIELDS, rows)
        with self.assertRaisesRegex(aliases.ImportErrorDetail, "invalid identity"):
            aliases.prepare(self.source, self.mapping)

    def test_numeric_and_unicode_only_aliases_fail_closed(self):
        self.given(source_rows=[
            {"book_id": "10", "public_id": "777", "slug": "Русский-роман"},
        ], map_rows=[
            {"newboo_book_id": "10", "tor_book_id": "42", "tor_source_identity": "source:10", "tor_target_language": "ru", "reviewed": "yes"},
        ])
        with self.assertRaisesRegex(aliases.ImportErrorDetail, "no safe nonnumeric alias"):
            aliases.prepare(self.source, self.mapping)

    def test_unmapped_source_and_duplicate_target_fail_closed(self):
        self.given()
        self.csv(self.mapping, aliases.MAP_FIELDS, [
            {"newboo_book_id": "888", "tor_book_id": "42", "tor_source_identity": "source:10", "tor_target_language": "ru", "reviewed": "yes"},
        ])
        with self.assertRaisesRegex(aliases.ImportErrorDetail, "absent from export"):
            aliases.prepare(self.source, self.mapping)
        self.csv(self.mapping, aliases.MAP_FIELDS, [
            {"newboo_book_id": "10", "tor_book_id": "42", "tor_source_identity": "source:10", "tor_target_language": "ru", "reviewed": "yes"},
            {"newboo_book_id": "11", "tor_book_id": "42", "tor_source_identity": "source:11", "tor_target_language": "ru", "reviewed": "yes"},
        ])
        with self.assertRaisesRegex(aliases.ImportErrorDetail, "repeated source or TOR Book"):
            aliases.prepare(self.source, self.mapping)

    def test_trailing_dot_does_not_create_unreachable_alias(self):
        self.given(source_rows=[
            {"book_id": "10", "public_id": "Valid-Key", "slug": "alias.v2."},
        ], map_rows=[
            {"newboo_book_id": "10", "tor_book_id": "42", "tor_source_identity": "source:10", "tor_target_language": "ru", "reviewed": "yes"},
        ])
        entries, unsupported = aliases.prepare(self.source, self.mapping)
        self.assertEqual([alias for alias, _ in entries], ["valid-key"])
        self.assertEqual(unsupported, [(10, "slug", "alias.v2.")])

    def test_check_only_does_not_write_sql(self):
        self.given()
        output = Path(self.temp.name) / "out.sql"
        self.assertEqual(aliases.main(["--source-export", str(self.source), "--reviewed-map", str(self.mapping), "--check-only", "--output-sql", str(output)]), 0)
        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
