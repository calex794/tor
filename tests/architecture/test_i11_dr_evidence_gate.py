"""Offline DROP-08c evidence gate negative controls (no backup or network access)."""
from pathlib import Path
import json
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from i11_dr_evidence_gate import (
    REQUIRED_REFS, REQUIRED_TRUE, Refused, load_report, review_report,
)


def complete_fixture():
    report = {
        "schema_version": 2,
        "status": "REVIEW_READY",
        "source_sha": "a" * 40,
        "postgres_version": "18.2",
        "pgbackrest_version": "2.54.2",
        "source_inventory_manifest_sha256": "b" * 64,
        "t_loss_utc": "2026-11-18T10:00:00Z",
        "t_latest_source_commit_utc": "2026-11-18T09:59:55Z",
        "t_latest_restored_commit_utc": "2026-11-18T09:59:00Z",
        "t_recovered_service_utc": "2026-11-18T10:40:00Z",
        "measured_rpo_seconds": 55,
        "measured_rto_seconds": 2400,
    }
    report.update({name: True for name in REQUIRED_TRUE})
    report.update({name: f"VLT-{name}" for name in REQUIRED_REFS})
    return report


class I11DrEvidenceGateTests(unittest.TestCase):
    def test_review_ready_is_only_schema_and_clock_eligibility(self):
        report = complete_fixture()
        self.assertEqual(review_report(report), (55, 2400))
        self.assertIn("saved_free_v1_after_v2_verified", REQUIRED_TRUE)
        self.assertIn("layer_free_v1_after_deletion_verified", REQUIRED_TRUE)
        self.assertIn("layer_revocation_denial_verified", REQUIRED_TRUE)

    def test_missing_or_fake_historical_rights_fail(self):
        for field in ("saved_free_v1_after_v2_verified",
                      "layer_free_v1_after_deletion_verified",
                      "layer_revocation_denial_verified",
                      "historical_free_receipts_verified",
                      "purchased_v1_after_v2_verified",
                      "wal_post_base_backup_replay_verified",
                      "controlled_wal_witness_verified",
                      "production_untouched",
                      "no_live_s3_fallback_verified"):
            for bad in (False, None, 1, "true"):
                report = complete_fixture()
                report[field] = bad
                with self.subTest(field=field, bad=bad), self.assertRaisesRegex(Refused, "required independent DR control"):
                    review_report(report)

    def test_incomplete_extra_or_self_approved_report_fails(self):
        for edit in (
            lambda r: r.pop("saved_free_v1_after_v2_verified"),
            lambda r: r.update({"unknown": "yes"}),
            lambda r: r.update({"status": "ACCEPTED"}),
            lambda r: r.update({"status": "NOT_ACCEPTED"}),
            lambda r: r.update({"schema_version": True}),
        ):
            report = complete_fixture()
            edit(report)
            with self.subTest(report=report.get("status")), self.assertRaises(Refused):
                review_report(report)

    def test_v2_requires_explicit_recovery_startup_and_both_key_epochs(self):
        fields = (
            "restored_pg_startup_isolation_verified",
            "exclusive_pitr_pause_before_promotion_verified",
            "historical_key_inventory_verified",
            "purchased_editions_kek_v1_v2_decryption_verified",
            "historical_kek_v2_missing_or_replaced_denial_verified",
        )
        for field in fields:
            self.assertIn(field, REQUIRED_TRUE)
            for bad in (False, None, 1, "true"):
                report = complete_fixture()
                report[field] = bad
                with self.subTest(field=field, bad=bad), self.assertRaises(Refused):
                    review_report(report)
            report = complete_fixture()
            del report[field]
            with self.subTest(missing=field), self.assertRaises(Refused):
                review_report(report)
        legacy = complete_fixture()
        legacy["schema_version"] = 1
        for field in fields:
            del legacy[field]
        with self.assertRaises(Refused):
            review_report(legacy)
        old_version = complete_fixture()
        old_version["schema_version"] = 1
        with self.assertRaisesRegex(Refused, "unsupported DR evidence schema"):
            review_report(old_version)

    def test_runbook_template_matches_gate_and_is_not_accepted(self):
        runbook = (ROOT / "docs/infra/i11-independent-dr-acceptance.md").read_text(
            encoding="utf-8"
        )
        template = json.loads(runbook.split("~~~json\n", 1)[1].split("\n~~~", 1)[0])
        self.assertEqual(set(template), set(complete_fixture()))
        self.assertEqual(template["schema_version"], 2)
        self.assertEqual(template["status"], "NOT_ACCEPTED")
        self.assertTrue(all(template[field] is None for field in (
            "restored_pg_startup_isolation_verified",
            "exclusive_pitr_pause_before_promotion_verified",
            "historical_key_inventory_verified",
            "purchased_editions_kek_v1_v2_decryption_verified",
            "historical_kek_v2_missing_or_replaced_denial_verified",
        )))
        with self.assertRaises(Refused):
            review_report(template)

    def test_evidence_refs_and_source_provenance_required(self):
        for field in REQUIRED_REFS:
            for bad in (None, "", "https://secret.example", "raw password", "a" * 129):
                report = complete_fixture()
                report[field] = bad
                with self.subTest(field=field, bad=bad), self.assertRaisesRegex(Refused, "evidence reference"):
                    review_report(report)
        for field, bad in (
            ("source_sha", "a" * 39),
            ("postgres_version", "17.9"),
            ("pgbackrest_version", ""),
            ("source_inventory_manifest_sha256", "0" * 64),
        ):
            report = complete_fixture()
            report[field] = bad
            with self.subTest(field=field), self.assertRaises(Refused):
                review_report(report)

    def test_rpo_rto_are_calculated_not_claimed(self):
        for field, bad in (
            ("measured_rpo_seconds", 0),
            ("measured_rto_seconds", 0),
            ("measured_rpo_seconds", True),
            ("measured_rto_seconds", "2400"),
            ("t_latest_source_commit_utc", "2026-11-18T10:00:01Z"),
            ("t_latest_restored_commit_utc", "2026-11-18T10:00:00Z"),
            ("t_recovered_service_utc", "2026-11-18T09:59:59Z"),
            ("t_loss_utc", "2026-11-18T10:00:00+00:00"),
            ("t_recovered_service_utc", "2026-02-30T10:00:00Z"),
        ):
            report = complete_fixture()
            report[field] = bad
            with self.subTest(field=field, bad=bad), self.assertRaises(Refused):
                review_report(report)

    def test_rpo_and_rto_limits(self):
        report = complete_fixture()
        report["t_latest_restored_commit_utc"] = "2026-11-18T09:40:00Z"
        report["measured_rpo_seconds"] = 1195
        with self.assertRaisesRegex(Refused, "outside operator targets"):
            review_report(report)
        report = complete_fixture()
        report["t_recovered_service_utc"] = "2026-11-18T14:30:00Z"
        report["measured_rto_seconds"] = 16200
        with self.assertRaisesRegex(Refused, "outside operator targets"):
            review_report(report)

    def test_json_rejects_duplicates_symlink_and_malformed(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            file = root / "report.json"
            file.write_text('{"a":1,"a":2}')
            with self.assertRaisesRegex(Refused, "duplicate"):
                load_report(file)
            file.write_text("{invalid")
            with self.assertRaisesRegex(Refused, "invalid"):
                load_report(file)
            file.write_text(json.dumps(complete_fixture()))
            self.assertEqual(review_report(load_report(file)), (55, 2400))
            link = root / "link.json"
            link.symlink_to(file)
            with self.assertRaisesRegex(Refused, "regular local report"):
                load_report(link)

    def test_cli_success_never_claims_dr_accepted(self):
        with tempfile.TemporaryDirectory() as folder:
            file = Path(folder) / "report.json"
            file.write_text(json.dumps(complete_fixture()))
            cmd = [sys.executable, str(ROOT / "tools/i11_dr_evidence_gate.py"),
                   "--report", str(file)]
            ok = subprocess.run(cmd, capture_output=True, text=True, check=False)
            self.assertEqual(ok.returncode, 0, ok.stderr)
            self.assertIn("TOR_I11_DR_EVIDENCE_REVIEW_READY", ok.stdout)
            self.assertIn("independent_operator_verification_required=true", ok.stdout)
            file.write_text(json.dumps({"status": "ACCEPTED"}))
            refused = subprocess.run(cmd, capture_output=True, text=True, check=False)
            self.assertEqual(refused.returncode, 2)
            self.assertIn("TOR_I11_DR_EVIDENCE_REFUSED:", refused.stderr)


if __name__ == "__main__":
    unittest.main()
