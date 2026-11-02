"""Public-only export of a complete unresolved taxonomy review queue."""
import contextlib
import csv
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import taxonomy_review_export as reviewer


class TaxonomyReviewExportTests(unittest.TestCase):
    URL = "postgresql://tor_migrator:ci@127.0.0.1:5432/tor_ci"

    @staticmethod
    def sample(label="Система", total=1):
        return {
            "kind": "tag", "normalized_key": "система",
            "review_label": label, "public_uses": 4, "spelling_count": 2,
            "term_id": 23, "status": "candidate",
            "raw_spellings": [
                {"label": "Система", "uses": 3},
                {"label": "СИСТЕМА", "uses": 1},
            ],
            "total_unresolved": total,
        }

    def test_query_is_single_public_only_read_only_snapshot(self):
        self.assertIn("BEGIN TRANSACTION READ ONLY;", reviewer.REVIEW_SQL)
        self.assertIn("FROM public_catalog_facets", reviewer.REVIEW_SQL)
        self.assertNotIn("FROM books", reviewer.REVIEW_SQL)
        self.assertIn("book_taxonomy_key(name)", reviewer.REVIEW_SQL)
        self.assertIn("count(*) OVER ()", reviewer.REVIEW_SQL)
        self.assertIn("active.status='active'", reviewer.REVIEW_SQL)
        with mock.patch.object(
            reviewer, "run_sql", return_value=json.dumps(self.sample(), ensure_ascii=False)
        ) as sql:
            rows = reviewer.fetch_review(self.URL)
        self.assertEqual(len(rows), 1)
        sql.assert_called_once_with(self.URL, reviewer.REVIEW_SQL, limit="10000")

    def test_review_csv_is_not_importable_without_editor_decisions(self):
        with tempfile.TemporaryDirectory() as root:
            dest = Path(root) / "review.csv"
            reviewer.export_review([self.sample()], dest)
            with dest.open(encoding="utf-8-sig", newline="") as file:
                reader = csv.DictReader(file)
                self.assertEqual(reader.fieldnames, list(reviewer.REVIEW_COLUMNS))
                rows = list(reader)
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0]["review_label"], "Система")
            self.assertEqual(rows[0]["status"], "candidate")
            self.assertEqual(rows[0]["term_id"], "23")
            self.assertEqual(rows[0]["decision"], "")
            self.assertEqual(rows[0]["approved_label"], "")
            self.assertEqual(rows[0]["approved_aliases"], "")
            self.assertEqual(json.loads(rows[0]["raw_spellings_json"]), self.sample()["raw_spellings"])

    def test_formula_payloads_are_inert_in_review_csv(self):
        with tempfile.TemporaryDirectory() as root:
            dest = Path(root) / "review.csv"
            row = self.sample(" =HYPERLINK(\"https://evil.invalid\")")
            row["normalized_key"] = "-10+20"
            reviewer.export_review([row], dest)
            with dest.open(encoding="utf-8-sig", newline="") as file:
                record = next(csv.DictReader(file))
            self.assertTrue(record["review_label"].startswith("' "))
            self.assertEqual(record["normalized_key"], "'-10+20")
            self.assertTrue(record["raw_spellings_json"].startswith("["))

    def test_refuse_incomplete_snapshot_without_creating_file(self):
        with tempfile.TemporaryDirectory() as root:
            dest = Path(root) / "review.csv"
            with mock.patch.dict(os.environ, {"TOR_TAXONOMY_ADMIN_URL": self.URL}):
                with mock.patch.object(
                    reviewer, "run_sql",
                    return_value=json.dumps(self.sample(total=15001))
                ):
                    with contextlib.redirect_stderr(io.StringIO()) as err:
                        result = reviewer.main([
                            "--output", str(dest), "--max-terms", "10000"
                        ])
            self.assertEqual(result, 2)
            self.assertFalse(dest.exists())
            self.assertIn("incomplete export", err.getvalue())

    def test_refuse_existing_output_and_non_curator_role(self):
        with tempfile.TemporaryDirectory() as root:
            dest = Path(root) / "review.csv"
            dest.write_text("do not overwrite", encoding="utf-8")
            with mock.patch.dict(os.environ, {"TOR_TAXONOMY_ADMIN_URL": self.URL}):
                with mock.patch.object(
                    reviewer, "run_sql", return_value=json.dumps(self.sample())
                ):
                    with contextlib.redirect_stderr(io.StringIO()):
                        result = reviewer.main(["--output", str(dest)])
            self.assertEqual(result, 2)
            self.assertEqual(dest.read_text(encoding="utf-8"), "do not overwrite")

            with mock.patch.dict(os.environ, {
                "TOR_TAXONOMY_ADMIN_URL":
                    "postgresql://tor_app:ci@127.0.0.1:5432/tor_ci"
            }):
                with mock.patch.object(reviewer, "run_sql") as sql:
                    with contextlib.redirect_stderr(io.StringIO()):
                        result = reviewer.main([
                            "--output", str(Path(root) / "new.csv")
                        ])
                    sql.assert_not_called()
            self.assertEqual(result, 2)

    def test_reject_corrupt_or_truncated_results(self):
        for bad in [
            '{"total_unresolved":2,"kind":"tag","review_label":"tag"}',
            '{"total_unresolved":1,"kind":"invalid","review_label":"tag"}',
            '{"total_unresolved":1,"kind":"tag","review_label":"tag"}',
        ]:
            with self.subTest(bad=bad):
                with mock.patch.object(reviewer, "run_sql", return_value=bad):
                    with self.assertRaises(ValueError):
                        reviewer.fetch_review(self.URL)


if __name__ == "__main__":
    unittest.main()
