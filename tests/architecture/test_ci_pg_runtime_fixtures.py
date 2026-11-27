"""REUSE-13E-b: keep the disposable PG fixtures, grants and order intact."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"
FIXTURES = ROOT / "tools/ci_pg_runtime_fixtures"
STEPS = (
    ("Seed visible and hidden rows", "books", "tor_app", "tor-ci-app"),
    ("Seed G-07/G-09 owned cover job fixtures", "cover_jobs", "tor_app", "tor-ci-app"),
    ("Seed published free and protected chapters", "chapters", "tor_app", "tor-ci-app"),
    ("Advance explicit-ID fixture sequences as schema owner", "sequences", "tor_migrator", "tor-ci-migrate"),
)


class SharedPgRuntimeFixturesTests(unittest.TestCase):
    def test_same_four_ordered_steps_keep_role_and_fail_closed_psql(self):
        for filename in ("tor-integration.yml", "tor-pg-runtime.yml"):
            with self.subTest(filename=filename):
                source = (WORKFLOWS / filename).read_text(encoding="utf-8")
                job = re.split(r"\n  [a-z][a-z0-9-]+:\n",
                               source.split("\n  postgres-runtime:\n", 1)[1],
                               maxsplit=1)[0]
                previous = job.index("Installed 190 to current analytics retirement")
                for name, fixture, role, password in STEPS:
                    path = f"tools/ci_pg_runtime_fixtures/{fixture}.sql"
                    step = (f"      - name: {name}\n"
                            "        run: |\n")
                    self.assertEqual(job.count(step), 1)
                    start = job.index(step)
                    body = job[start:].split("\n      - name:", 1)[0]
                    self.assertEqual(body.count(
                        f"PGPASSWORD={password} PGUSER={role} PGDATABASE=tor_ci "
                        f"psql -X -v ON_ERROR_STOP=1 < {path}"), 1)
                    self.assertEqual(job.count(path), 1)
                    self.assertNotIn("<<'SQL'", body)
                    self.assertNotIn("continue-on-error:", body)
                    self.assertNotIn("        if:", body)
                    self.assertGreater(start, previous)
                    previous = start
                self.assertLess(previous, job.index("Database and HTTP integration"))
                self.assertIn("PGHOST: 127.0.0.1", job)
                self.assertIn("image: postgres:18", job)

    def test_fixture_inventory_visibility_and_schema_owner_sequence_repairs(self):
        self.assertEqual({p.stem for p in FIXTURES.glob("*.sql")},
                         {fixture for _, fixture, _, _ in STEPS})
        books = (FIXTURES / "books.sql").read_text(encoding="utf-8")
        for marker in ("(1,'ci:1','ru','TOR CI public','public','basic')",
                       "(2,'ci:2','ru','TOR CI hidden','draft','basic')",
                       "(3,'ci:3','ru','TOR CI held','public','basic')",
                       "UPDATE books SET admin_hold=true WHERE id=3;",
                       "WHERE id IN (1,2,3);"):
            self.assertIn(marker, books)
        covers = (FIXTURES / "cover_jobs.sql").read_text(encoding="utf-8")
        self.assertEqual(covers.count("generate_series(901,905)"), 2)
        self.assertIn("OVERRIDING SYSTEM VALUE", covers)
        self.assertIn("owner_user_id", covers)
        chapters = (FIXTURES / "chapters.sql").read_text(encoding="utf-8")
        for marker in ("PRIVATE-DRAFT-MARKER", "PRIVATE-PAID-MARKER",
                       "PRIVATE-BOOK-MARKER", "PRIVATE-HOLD-MARKER",
                       "PRIVATE-FUTURE-MARKER", "PRIVATE-MODERATED-MARKER",
                       "now() + INTERVAL '1 day'",
                       "UPDATE chapters SET admin_hold=true WHERE id=107;",
                       "INSERT INTO chapter_publications(chapter_id, version_id)"):
            self.assertIn(marker, chapters)
        self.assertEqual(re.findall(r"(?m)^\s*\((10[1-8]), [123], \d+,", chapters),
                         [str(n) for n in range(101, 109)])
        self.assertEqual(re.findall(r"(?m)^\s*\((20[1-8]), 10[1-8], 1,", chapters),
                         [str(n) for n in range(201, 209)])
        seq = (FIXTURES / "sequences.sql").read_text(encoding="utf-8")
        self.assertEqual(re.findall(r"pg_get_serial_sequence\('([^']+)','id'\)", seq),
                         ["users", "books", "chapters", "chapter_versions"])
        for table in ("users", "books", "chapters", "chapter_versions"):
            self.assertIn(f"GREATEST((SELECT max(id) FROM {table}),1), true);", seq)
        for path in FIXTURES.glob("*.sql"):
            sql = path.read_text(encoding="utf-8")
            self.assertNotRegex(sql, r"(?im)^\s*(?:DROP|TRUNCATE|DELETE|ALTER|GRANT|REVOKE)\s")
            self.assertNotRegex(sql, r"(?m)^\s*\\")
            self.assertTrue(sql.endswith("\n"))


if __name__ == "__main__":
    unittest.main()
