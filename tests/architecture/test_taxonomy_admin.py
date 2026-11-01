"""Read-only taxonomy lookup and paginated unmatched list."""
import contextlib
import io
import os
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import taxonomy_admin as admin


class TaxonomyAdminReadTests(unittest.TestCase):
    URL = "postgresql://tor_migrator:ci@127.0.0.1:5432/tor_ci"

    def call(self, argv):
        with mock.patch.dict(os.environ, {"TOR_TAXONOMY_ADMIN_URL": self.URL}):
            with mock.patch.object(admin, "run_sql", return_value="test-result") as query:
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    with contextlib.redirect_stderr(io.StringIO()):
                        result = admin.main(argv)
                return result, output.getvalue(), query

    def test_lookup_is_read_only_and_passes_unquoted_prefix(self):
        result, stdout, query = self.call(["lookup", "tag", "Wizard's", "--limit", "7"])
        self.assertEqual(result, 0)
        self.assertEqual(stdout, "test-result\n")
        query.assert_called_once_with(
            self.URL, admin.LOOKUP_SQL, kind="tag", query="Wizard's", limit="7"
        )
        self.assertIn("admin_taxonomy_key(:'query')", admin.LOOKUP_SQL)
        self.assertIn("book_taxonomy_aliases", admin.LOOKUP_SQL)

    def test_lookup_rejects_wildcards_and_control_characters(self):
        for value in ["", "  ", "a%b", "a_b", "a" + chr(92) + "b", "a\nb", "x" * 81]:
            with self.subTest(value=value):
                result, _, query = self.call(["lookup", "tag", value])
                self.assertEqual(result, 2)
                query.assert_not_called()

    def test_unmapped_uses_bounded_offset(self):
        result, _, query = self.call(["unmapped", "--limit", "50", "--offset", "500"])
        self.assertEqual(result, 0)
        query.assert_called_once_with(
            self.URL, admin.READ_ONLY, limit="50", offset="500"
        )
        self.assertIn("OFFSET :'offset'::integer", admin.READ_ONLY)

    def test_invalid_pagination_does_not_touch_db(self):
        for offset in ["-1", "1000001"]:
            with self.subTest(offset=offset):
                result, _, query = self.call(["unmapped", "--offset", offset])
                self.assertEqual(result, 2)
                query.assert_not_called()

    def test_lookup_rejects_non_migrator_url(self):
        with mock.patch.dict(
            os.environ,
            {"TOR_TAXONOMY_ADMIN_URL": "postgresql://tor_app:ci@localhost/tor_ci"},
        ):
            with mock.patch.object(admin, "run_sql") as query:
                with contextlib.redirect_stderr(io.StringIO()):
                    result = admin.main(["lookup", "tag", "сист"])
                self.assertEqual(result, 2)
                query.assert_not_called()


if __name__ == "__main__":
    unittest.main()
