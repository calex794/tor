"""The existing S334 recovery witness also requires real D-08 postings."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class S334PaidRestoreWiring(unittest.TestCase):
    def test_seed_publishes_terms_and_enables_actual_finance_writer(self):
        rust = (ROOT / "crates/web/tests/s334_paid_restore_pg_s3.rs").read_text()
        self.assertIn("INSERT INTO finance_revenue_terms", rust)
        self.assertIn("INSERT INTO finance_revenue_term_states", rust)
        self.assertIn("'ci-s334-finance-v1',2000,'ci-s334-refund-7d-v1',604800", rust)
        self.assertEqual(rust.count(".with_creator_posting(true)"), 2)
        self.assertLess(rust.index("INSERT INTO finance_revenue_term_states"),
                        rust.index("let purchase = PgCheckout::new"))

    def test_witness_requires_financial_records_and_preserves_existing_controls(self):
        sql = (ROOT / "tools/i11_s334_paid_recovery_facts.sql").read_text()
        self.assertIn("current_setting('transaction_read_only') <> 'on'", sql)
        self.assertNotRegex(sql, r"(?im)^\s*(INSERT|UPDATE|DELETE|TRUNCATE|DROP|ALTER)\b")
        for marker in ("chapter_purchase_commands", "library_purchased_chapters",
                       "finance_chapter_postings", "finance_platform_credits",
                       "finance_creator_earnings", "fp.platform_fee_minor=35",
                       "fp.creator_pool_minor=140", "e.amount_minor=140",
                       "e.clear_at=fp.refund_deadline_at", "q.expected_version_id=version_ref",
                       "S334 encrypted purchase finance witness differs"):
            self.assertIn(marker, sql)
        self.assertIn("sha256(convert_to(to_jsonb(s)::text,'UTF8'))", sql)
        self.assertIn("sha256(convert_to(to_jsonb(v)::text,'UTF8'))", sql)
        self.assertEqual(sql.count("tor_purchase_version_readable("), 7)
        self.assertIn("S334 unexpected pre-refund revocation", sql)
        shell = (ROOT / "tools/tor_s334_paid_restore_smoke.sh").read_text()
        self.assertIn('i11_s334_paid_recovery_facts.sql', shell)
        self.assertIn('"$restored_paid_snapshot" == "$source_paid_snapshot"', shell)
        self.assertIn('$(paid_snapshot "$TEST_DATABASE_URL")" == "$source_paid_snapshot"', shell)


if __name__ == "__main__":
    unittest.main()
