"""PERF-04: prevent the read-only SQL profile from diverging from F-03 search."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROFILE = ROOT / "tests/load/catalog_sql_profile.sql"
CATALOG = ROOT / "crates/search/src/repositories/catalog.rs"


def compact(value):
    return re.sub(r"\s+", "", value)


class DenseSearchPlanProfile(unittest.TestCase):
    def setUp(self):
        self.profile = PROFILE.read_text(encoding="utf-8")
        self.runtime = CATALOG.read_text(encoding="utf-8")

    def test_profile_is_select_only_and_reviewable(self):
        # Source SQL has no DML/DDL; EXPLAIN ANALYZE only executes SELECT.
        # A future operator must never point this profiling job at production.
        self.assertNotRegex(
            self.profile,
            r"(?im)^[ \t]*(?:CREATE|DROP|ALTER|TRUNCATE|INSERT|UPDATE|DELETE|GRANT|REVOKE)\b",
        )
        for marker in ("PERF04_FTS_DENSE_LOAD_EXPLAIN",
                       "PERF04_FTS_RARE_9999_EXPLAIN"):
            self.assertEqual(self.profile.count(marker), 1)
        self.assertIn("PREPARE perf04_public_fts(", self.profile)
        self.assertIn("DEALLOCATE perf04_public_fts;", self.profile)

    def test_normalized_fts_contract_matches_runtime(self):
        prof = compact(self.profile)
        rust = compact(self.runtime)
        expressions = (
            "to_tsvector('simple'::regconfig,translate(lower(normalize(title,NFKC)),'ё','е'))",
            "to_tsvector('simple'::regconfig,translate(lower(normalize(author_name,NFKC)),'ё','е'))",
            "plainto_tsquery('simple'::regconfig,translate(lower(normalize($1::text,NFKC)),'ё','е'))",
            "visibility='public'ANDvisibility_status='active'",
            "access_level='basic'ANDlifecycle_status<>'pending_deletion'",
            "ORDERBYidASCLIMIT$3",
        )
        for expression in expressions:
            with self.subTest(expression=expression):
                self.assertIn(expression, prof)
                self.assertIn(expression, rust)
        self.assertEqual(prof.count("EXPLAIN(ANALYZE,BUFFERS,SETTINGS)"), 2)
        self.assertIn("EXECUTEperf04_public_fts('Load',0,21,NULL,NULL,NULL,NULL);", prof)
        self.assertIn("EXECUTEperf04_public_fts('9999',0,21,NULL,NULL,NULL,NULL);", prof)


if __name__ == "__main__":
    unittest.main()
