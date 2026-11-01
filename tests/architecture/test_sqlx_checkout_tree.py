"""Regression tests for fail-closed merged-checkout SQLx version checks."""
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_sqlx_checkout_tree import MigrationTreeError, check_migration_tree, main


class SqlxCheckoutTreeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / "migrations"
        self.root.mkdir()

    def write(self, name, sql):
        path = self.root / name
        path.write_bytes(sql)
        return path

    def test_current_main_tree_is_valid(self):
        self.assertGreaterEqual(check_migration_tree(ROOT / "migrations"), 68)

    def test_unique_versions_with_distinct_sql_pass(self):
        self.write("202610310292_book_engagement_integrity.sql", b"SELECT 1;\n")
        self.write("202610310293_d06_refund_admin_step_up.sql", b"SELECT 2;\n")
        self.assertEqual(check_migration_tree(self.root), 2)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(main([str(self.root)]), 0)
        self.assertIn("2 unique versions", output.getvalue())

    def test_main_and_pr_348_duplicate_293_fails(self):
        self.write("202610310293_d06_refund_admin_step_up.sql", b"SELECT 1;\n")
        self.write("202610310293_author_subscriber_discounts.sql", b"SELECT 2;\n")
        with self.assertRaisesRegex(MigrationTreeError, "Duplicate SQLx version 202610310293"):
            check_migration_tree(self.root)
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.assertEqual(main([str(self.root)]), 2)
        self.assertIn("SQLx checkout tree BLOCK", stderr.getvalue())

    def test_same_sql_bytes_under_two_versions_fails(self):
        sql = b"ALTER TABLE users ADD COLUMN example boolean;\n"
        self.write("202610310292_example.sql", sql)
        self.write("202610310294_example_again.sql", sql)
        with self.assertRaisesRegex(MigrationTreeError, "Identical SQLx content"):
            check_migration_tree(self.root)

    def test_empty_and_malformed_migrations_fail_closed(self):
        with self.assertRaisesRegex(MigrationTreeError, "empty"):
            check_migration_tree(self.root)
        self.write("20261031029_bad_length.sql", b"SELECT 1;\n")
        with self.assertRaisesRegex(MigrationTreeError, "Invalid SQLx migration file"):
            check_migration_tree(self.root)
        (self.root / "20261031029_bad_length.sql").unlink()
        self.write("202610310292_empty.sql", b"")
        with self.assertRaisesRegex(MigrationTreeError, "Empty SQLx migration"):
            check_migration_tree(self.root)

    def test_symlink_and_invalid_utf8_fail_closed(self):
        safe = self.write("202610310292_safe.sql", b"SELECT 1;\n")
        link = self.root / "202610310293_symlink.sql"
        link.symlink_to(safe)
        with self.assertRaisesRegex(MigrationTreeError, "Invalid SQLx migration file"):
            check_migration_tree(self.root)
        link.unlink()
        self.write("202610310293_bad_unicode.sql", b"\xff\xfe")
        with self.assertRaisesRegex(MigrationTreeError, "Unreadable SQLx migration"):
            check_migration_tree(self.root)


if __name__ == "__main__":
    unittest.main()
