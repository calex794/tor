"""FND-01 regression: existing SQLx migration identities never change."""

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_frozen_migrations import (
    MigrationHistoryError,
    check_frozen_migrations,
    main,
)


class FrozenMigrationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        self.git("init", "-q")
        self.git("config", "user.name", "TOR CI")
        self.git("config", "user.email", "ci@example.invalid")
        self.migrations = self.repo / "migrations"
        self.migrations.mkdir()
        self.write("202610120001_create_accounts.sql", "SELECT 1;\n")
        self.write("202610120002_create_books.sql", "SELECT 2;\n")
        self.commit()
        self.base = self.git("rev-parse", "HEAD").strip()

    def git(self, *args):
        return subprocess.check_output(
            ["git", "-C", str(self.repo), *args], stderr=subprocess.STDOUT, text=True
        )

    def write(self, filename, text):
        (self.migrations / filename).write_text(text, encoding="utf-8")

    def commit(self):
        self.git("add", "-A")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "test fixture")

    def test_additive_migrations_can_be_committed(self):
        self.write("202610120003_additive.sql", "SELECT 3;\n")
        self.commit()
        self.assertEqual(check_frozen_migrations(self.repo, self.base), (2, 1))
        self.assertEqual(main(["--repo", str(self.repo), "--base-ref", self.base]), 0)

    def test_migration_sql_cannot_be_changed(self):
        self.write("202610120001_create_accounts.sql", "SELECT 99;\n")
        self.commit()
        with self.assertRaisesRegex(MigrationHistoryError, "modified"):
            check_frozen_migrations(self.repo, self.base)

    def test_migration_cannot_be_deleted(self):
        (self.migrations / "202610120002_create_books.sql").unlink()
        self.commit()
        with self.assertRaisesRegex(MigrationHistoryError, "removed or renamed"):
            check_frozen_migrations(self.repo, self.base)

    def test_migration_cannot_be_renumbered_after_merge(self):
        (self.migrations / "202610120002_create_books.sql").rename(
            self.migrations / "202610120003_create_books.sql"
        )
        self.commit()
        with self.assertRaisesRegex(MigrationHistoryError, "removed or renamed"):
            check_frozen_migrations(self.repo, self.base)

    def test_duplicate_version_fails_closed(self):
        self.write("202610120002_other_name.sql", "SELECT 123;\n")
        self.commit()
        with self.assertRaisesRegex(MigrationHistoryError, "Duplicate SQLx version"):
            check_frozen_migrations(self.repo, self.base)

    def test_sql_symlink_fails_closed(self):
        (self.migrations / "202610120003_link.sql").symlink_to(
            "202610120001_create_accounts.sql"
        )
        self.commit()
        with self.assertRaisesRegex(MigrationHistoryError, "Unsafe SQLx migration"):
            check_frozen_migrations(self.repo, self.base)

    def test_missing_base_fails_closed(self):
        with self.assertRaisesRegex(MigrationHistoryError, "Cannot inspect"):
            check_frozen_migrations(self.repo, "0000000000000000000000000000000000000000")

    def test_base_unchanged_does_not_require_a_new_migration(self):
        self.assertEqual(check_frozen_migrations(self.repo, self.base), (2, 0))


if __name__ == "__main__":
    unittest.main()
