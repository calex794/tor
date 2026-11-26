"""Keep the S334 paid snapshot fail-closed and outside the backup under test."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class S334PaidRestoreWiring(unittest.TestCase):
    def test_read_only_exact_witness_precedes_backup_and_http(self):
        shell = (ROOT / "tools/tor_s334_paid_restore_smoke.sh").read_text()
        subprocess.run(["bash", "-n", str(ROOT / "tools/tor_s334_paid_restore_smoke.sh")], check=True)
        self.assertIn("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;", shell)
        self.assertLess(shell.index('source_paid="$(paid_snapshot'), shell.index('pg_dump --host='))
        self.assertLess(shell.index('pg_restore --host='), shell.index('restored_paid="$(paid_snapshot'))
        self.assertLess(shell.index('restored_paid="$(paid_snapshot'), shell.index('s3_34_verify_restored_paid_v1_and_rejected_keys'))
        self.assertIn('"$restored_paid" == "$source_paid"', shell)
        self.assertIn('S334 encrypted purchase entitlement witness differs', shell)
        self.assertIn('S334_EXACT_PAID_LEDGER_NEGATIVE_CONTROL_OK', shell)
        self.assertLess(shell.index('paid_grant_webcrypto_e2e.mjs'), shell.index('S334_EXACT_PAID_LEDGER_NEGATIVE_CONTROL_OK'))

    def test_witness_checks_real_entitlements_registry_and_finance(self):
        sql = (ROOT / "tools/i11_s334_paid_restore_facts.sql").read_text()
        self.assertIn("current_setting('transaction_read_only') <> 'on'", sql)
        self.assertNotRegex(sql, r"(?im)^\s*(INSERT|UPDATE|DELETE|TRUNCATE|DROP|ALTER)\b")
        for marker in ("tor_purchase_version_readable", "chapter_purchase_revocations",
                       "storage_objects", "chapter_purchase_commands", "library_purchased_chapters",
                       "finance_chapter_postings", "finance_platform_credits", "finance_creator_earnings",
                       "w.balance_minor=425", "e.delta_minor=-175",
                       "fp.platform_fee_minor=35", "fp.creator_pool_minor=140",
                       "content-kek-v1", "content-kek-v2"):
            self.assertIn(marker, sql)
        self.assertIn("sha256(convert_to(to_jsonb(o)::text,'UTF8'))", sql)
        self.assertIn("sha256(convert_to(to_jsonb(v)::text,'UTF8'))", sql)
        self.assertNotIn("'registry', (SELECT jsonb_agg(to_jsonb(o)", sql)


if __name__ == "__main__":
    unittest.main()
