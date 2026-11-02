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
        # The normalized prefix, not only raw input, must be checked.
        self.assertEqual(admin.LOOKUP_SQL.count("strpos(p.k,'%')=0"), 2)
        self.assertEqual(admin.LOOKUP_SQL.count("strpos(p.k,'_')=0"), 2)
        self.assertEqual(admin.LOOKUP_SQL.count("strpos(p.k,chr(92))=0"), 2)

    def test_nfkc_wildcards_are_rejected_by_query_not_mistaken_for_masks(self):
        # Fullwidth characters are valid raw input; SQL rejects their NFKC keys.
        for raw in ["％", "＿", "＼"]:
            with self.subTest(raw=raw):
                result, _, query = self.call(["lookup", "genre", raw])
                self.assertEqual(result, 0)
                query.assert_called_once_with(
                    self.URL, admin.LOOKUP_SQL,
                    kind="genre", query=raw, limit="40"
                )

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

    def test_unmapped_uses_public_projection_and_review_identity(self):
        sql = admin.READ_ONLY
        self.assertIn("FROM public_catalog_facets", sql)
        self.assertNotIn("FROM books", sql)
        self.assertIn("sum(f.reference_count)::bigint AS uses", sql)
        self.assertIn("count(*)::bigint AS spellings", sql)
        self.assertIn("coalesce(t.label,u.example_label) AS review_label", sql)
        self.assertIn("t.id AS term_id", sql)
        self.assertIn("coalesce(t.status,'not_staged') AS status", sql)
        self.assertIn("t.status='active'", sql)

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
