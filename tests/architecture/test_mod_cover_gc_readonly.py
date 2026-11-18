"""MOD-COVER-OPS-01: static guard for the read-only operator preview.

Actual PostgreSQL semantics are separately tested on disposable PG18 in CI.
"""
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SQL = (ROOT / "tools" / "mod_cover_gc_dry_run.sql").read_text(encoding="utf-8")


class CoverGcReadOnlyPreview(unittest.TestCase):
    def test_read_only_transaction_and_no_shell_or_ddl_dml(self):
        body = re.sub(r"--[^\n]*", "", SQL)
        self.assertIn(
            "BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY;",
            body,
        )
        self.assertIn("SET LOCAL statement_timeout", body)
        self.assertIn("SET LOCAL lock_timeout", body)
        self.assertEqual(len(re.findall(r"(?im)^\s*ROLLBACK\s*;", body)), 1)
        for forbidden in (r"\!", r"\gexec", r"\copy"):
            self.assertNotIn(forbidden, body)
        self.assertNotRegex(
            body,
            re.compile(
                r"\b(?:INSERT|UPDATE|DELETE|TRUNCATE|ALTER|DROP|CREATE|CALL|"
                r"COPY|GRANT|REVOKE|EXECUTE|DO)\b",
                flags=re.I,
            ),
        )

    def test_null_terminal_timestamp_cannot_become_eligible(self):
        # PostgreSQL uses three-valued logic: NOT(NULL) is NULL. The operator
        # preview must treat an unknown review timestamp as ineligible, just
        # like the guarded B cleanup worker.
        self.assertIn(
            ") IS NOT TRUE THEN 'retain_pending_recent_or_undated_review'",
            SQL,
        )
        self.assertNotIn(
            ") THEN 'retain_pending_recent_or_undated_review'",
            SQL.replace(
                ") IS NOT TRUE THEN 'retain_pending_recent_or_undated_review'",
                "",
            ),
        )

    def test_worker_guards_and_no_key_disclosure(self):
        for fragment in (
            "cover_private_variant_removals",
            "cover_private_source_removals",
            "book_cover_review_requests",
            "cover_upload_objects",
            "book_cover_slots",
            "cover_variant_jobs",
            "cover_variants",
            "r.decided_at<=now()-interval '7 days'",
            "r.superseded_at<=now()-interval '7 days'",
            "done.state<>'removed'",
            "NOT EXISTS",
            "ANOMALY_removed_with_live_reference",
            "retain_missing_review",
            "eligible_to_stage",
            "eligible_to_claim",
        ):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, SQL)
        self.assertIn("src.storage_key LIKE 'staging/covers/sources/%'", SQL)
        self.assertNotIn("storage_key AS internal_examples", SQL)
        self.assertNotIn("array_agg(storage_key", SQL.lower())
        self.assertEqual(SQL.count("AS internal_examples"), 2)
        self.assertEqual(SQL.count("[1:12]"), 2)


if __name__ == "__main__":
    unittest.main()
