"""RECOVERY-10a: keep real finance witness in the encrypted backup path."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class FinanceRestoreWiring(unittest.TestCase):
    def test_actual_checkout_snapshot_before_backup_and_after_restore(self):
        script = ROOT / "tools/tor_dr_s3_backup_smoke.sh"
        subprocess.run(["bash", "-n", str(script)], check=True)
        text = script.read_text()
        seed = "cargo test -p tor-commerce --test d08_finance_pg d_08_single_purchase_posts_exact_credit_and_hold_once"
        before = 'source_finance="$(finance_snapshot "$TEST_DATABASE_URL")"'
        after = 'restored_finance="$(finance_snapshot "$restored_url")"'
        self.assertEqual(text.count(seed), 1)
        self.assertIn("-- --ignored --exact --test-threads=1", text)
        self.assertLess(text.index(seed), text.index(before))
        self.assertLess(text.index(before), text.index("pg_dump --host="))
        self.assertLess(text.index("pg_restore --host="), text.index(after))
        self.assertLess(text.index(after), text.index("I11_RESTORED_CHECKOUT_FINANCE_OK"))
        self.assertIn('"$restored_finance" == "$source_finance"', text)
        self.assertIn('[[ "$(finance_snapshot "$TEST_DATABASE_URL")" == "$source_finance" ]]', text)
        self.assertIn('psql "$1" -X -q -A -t -v ON_ERROR_STOP=1 -f tools/i11_finance_restore_facts.sql', text)

    def test_witness_covers_exact_journal_and_postings_not_only_total(self):
        sql = (ROOT / "tools/i11_finance_restore_facts.sql").read_text()
        self.assertIn("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;", sql)
        for table in ("reader_wallet_accounts", "reader_wallet_entries",
                      "chapter_purchase_facts", "chapter_purchase_commands",
                      "finance_chapter_postings", "finance_platform_credits",
                      "finance_creator_earnings"):
            self.assertIn(table, sql)
        for check in ("balance_minor=750", "delta_minor=-250",
                      "gross_minor=250", "platform_fee_minor=50",
                      "creator_pool_minor=200", "pc.amount_minor=50",
                      "e.amount_minor=200", "INTO STRICT"):
            self.assertIn(check, sql)
        self.assertIn("RAISE EXCEPTION", sql)
        self.assertNotRegex(sql, r"(?im)^\s*(INSERT|UPDATE|DELETE|TRUNCATE|DROP|ALTER)\b")


if __name__ == "__main__":
    unittest.main()
