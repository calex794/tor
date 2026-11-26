"""Require recovery evidence to follow the actual scenario assertions."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class HistoricalRecoveryEvidenceTests(unittest.TestCase):
    def test_read_only_exact_snapshot_and_negative_control(self):
        sql = (ROOT / "tools/i11_s334_paid_recovery_facts.sql").read_text()
        self.assertIn("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY", sql)
        self.assertEqual(sql.count("tor_purchase_version_readable("), 7)
        for secret in ("encrypted_dek", "wrap_nonce", "object_nonce", "password_hash", "session_token"):
            self.assertNotIn(secret, sql)
        smoke = ROOT / "tools/tor_s334_paid_restore_smoke.sh"
        subprocess.run(["bash", "-n", str(smoke)], check=True)
        body = smoke.read_text()
        self.assertLess(body.index('source_paid_snapshot='), body.index('postgres:18 pg_dump'))
        self.assertLess(body.index('postgres:18 pg_restore'), body.index('restored_paid_snapshot='))
        self.assertLess(body.index('restored_paid_snapshot='), body.index('s3_34_verify_restored_paid_v1_and_rejected_keys'))
        self.assertIn('S334 unexpected pre-refund revocation', body)
        self.assertIn('$(paid_snapshot "$TEST_DATABASE_URL")" == "$source_paid_snapshot"', body)

    def test_new_markers_bound_to_existing_negative_controls(self):
        paid = (ROOT / "crates/web/tests/s334_paid_restore_pg_s3.rs").read_text()
        self.assertIn("[(buyer, purchase_v2_id), (buyer_v2, purchase_id)]", paid)
        self.assertLess(paid.index("INSERT INTO chapter_purchase_revocations"),
                        paid.index("DEPLOY07C_BOTH_RESTORED_REFUNDS_DENIED"))
        free = (ROOT / "crates/web/tests/fnd05_restored_free_rights_pg_s3.rs").read_text()
        self.assertIn("Some(rights.revoked.as_str())", free)
        self.assertLess(free.index("for token in [&rights.unrelated, &rights.revoked, &rights.saved]"),
                        free.index("DEPLOY07C_SAVED_FREE_LAYER_AND_REVOKED_DENIAL_OK"))
        pack = (ROOT / "tools/drop07_pack_restore_ci.sh").read_text()
        self.assertLess(pack.index("corrupt restored PACK passed the real frame verifier"),
                        pack.index("DEPLOY07C_CORRUPT_RESTORED_PACK_DENIED"))


if __name__ == "__main__":
    unittest.main()
