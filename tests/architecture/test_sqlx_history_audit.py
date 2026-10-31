"""Read-only SQLx history and parallel PR collision regression tests."""
import csv
import hashlib
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from audit_sqlx_history import (
    AuditError, evaluate_history, local_migrations, parse_inventory,
    read_from_psql, scan_conflicts, scan_cross_version_identical_sql,
    main as audit_main,
)


def inventory(rows):
    buffer = io.StringIO()
    writer = csv.DictWriter(
        buffer,
        fieldnames=["database_name", "version", "description", "checksum_sha384_hex", "success"],
    )
    writer.writeheader()
    for row in rows:
        writer.writerow(row)
    return buffer.getvalue()


def record(version, desc, checksum, success="t", database="tor_ci"):
    return {
        "database_name": database,
        "version": version,
        "description": desc,
        "checksum_sha384_hex": checksum,
        "success": success,
    }


class SQLxAuditTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.version = "202610290003"
        self.sql = b"CREATE TABLE purchases(id bigint);\n"
        (self.root / f"{self.version}_batch_chapter_purchases.sql").write_bytes(self.sql)
        self.expected = local_migrations(self.root)
        self.valid = record(
            self.version, "batch chapter purchases", hashlib.sha384(self.sql).hexdigest()
        )

    def assess(self, records):
        database, applied = parse_inventory(inventory(records))
        return evaluate_history(self.expected, database, applied)

    def test_matching_history_is_not_deployment_approval(self):
        result = self.assess([self.valid])
        self.assertEqual(result["status"], "NO_APPLIED_DRIFT_DETECTED")
        self.assertEqual(result["matched"], [self.version])
        self.assertIn("not a deploy approval", result["notice"])

    def test_same_version_different_sql_is_a_blocker(self):
        changed = dict(self.valid, checksum_sha384_hex="a" * 96)
        result = self.assess([changed])
        self.assertEqual(result["status"], "BLOCK")
        self.assertIn("checksum", result["blockers"][0])

    def test_applied_description_change_is_a_blocker(self):
        changed = dict(self.valid, description="layer refund policy snapshot")
        result = self.assess([changed])
        self.assertEqual(result["status"], "BLOCK")
        self.assertIn("description", result["blockers"][0])

    def test_failed_and_unknown_history_block(self):
        failed = dict(self.valid, success="f")
        self.assertEqual(self.assess([failed])["status"], "BLOCK")
        foreign = record("202610290004", "old layer grant", "b" * 96)
        self.assertEqual(self.assess([self.valid, foreign])["status"], "BLOCK")

    def test_duplicate_or_mixed_database_is_not_a_report(self):
        with self.assertRaises(AuditError):
            parse_inventory(inventory([self.valid, self.valid]))
        other = dict(self.valid, database_name="tor_secondary", version="202610290004")
        with self.assertRaises(AuditError):
            parse_inventory(inventory([self.valid, other]))

    def test_pending_lower_number_is_visible_not_misreported_as_used(self):
        (self.root / "202610290002_layer_access.sql").write_text("SELECT 1;\n")
        current = local_migrations(self.root)
        name, installed = parse_inventory(inventory([self.valid]))
        result = evaluate_history(current, name, installed)
        self.assertEqual(result["pending"], ["202610290002"])
        self.assertEqual(result["out_of_order_pending"], ["202610290002"])
        self.assertEqual(result["status"], "NO_APPLIED_DRIFT_DETECTED")

    def test_malformed_csv_fails_closed(self):
        with self.assertRaises(AuditError):
            parse_inventory("version,checksum\n3,x\n")
        with self.assertRaises(AuditError):
            parse_inventory(inventory([]))
        with self.assertRaises(AuditError):
            parse_inventory(inventory([dict(self.valid, checksum_sha384_hex="zz")]))

    def test_duplicate_numeric_local_version_fails_closed(self):
        (self.root / "202610290003_refund_policy.sql").write_text("SELECT 1;\n")
        with self.assertRaises(AuditError):
            local_migrations(self.root)

    def test_psql_requires_explicit_selection_and_never_calls_migrator(self):
        with patch.dict("os.environ", {"PGSERVICE": "", "PGDATABASE": ""}):
            with self.assertRaises(AuditError):
                read_from_psql()
        with patch.dict("os.environ", {"PGSERVICE": "readonly"}):
            with patch("subprocess.run") as run:
                run.return_value.returncode = 0
                run.return_value.stdout = inventory([self.valid])
                read_from_psql()
                argv = run.call_args.args[0]
                env = run.call_args.kwargs["env"]
                self.assertEqual(argv[0], "psql")
                self.assertIn("COPY (SELECT", argv[-1])
                self.assertIn("default_transaction_read_only=on", env["PGOPTIONS"])
                self.assertNotIn("tor-migrate", " ".join(argv))

    def test_open_pr_collision_by_version_or_modified_blob(self):
        a = "1" * 40
        b = "2" * 40
        main = [{"path": "migrations/202610290003_batch_chapter_purchases.sql", "sha": a}]
        pulls = [
            {"number": 216, "files": [
                {"filename": "migrations/202610290003_layer_refund_policy_snapshot.sql",
                 "sha": b, "status": "added"}]},
            {"number": 224, "files": [
                {"filename": "migrations/202610290003_batch_chapter_purchases.sql",
                 "sha": a, "status": "added"}]},
            {"number": 231, "files": [
                {"filename": "migrations/202610290005_direct_paid_grant_exposure.sql",
                 "sha": b, "status": "added"}]},
        ]
        conflicts = scan_conflicts(main, pulls)
        self.assertEqual([c["version"] for c in conflicts], ["202610290003"])
        self.assertEqual(len(conflicts[0]["variants"]), 2)
        self.assertEqual(conflicts[0]["variants"][0]["owners"], ["#224", "main"])

    def test_same_filename_changed_blob_and_deleted_paths(self):
        a = "1" * 40
        b = "2" * 40
        path = "migrations/202610280013_commerce_layers.sql"
        main = [{"path": path, "sha": a}]
        pulls = [{"number": 178, "files": [{"filename": path, "sha": b, "status": "modified"}]}]
        self.assertEqual(len(scan_conflicts(main, pulls)), 1)
        pulls[0]["files"][0]["status"] = "removed"
        self.assertEqual(scan_conflicts(main, pulls), [])


    def test_identical_sql_blob_under_two_versions_is_reported(self):
        blob = "a" * 40
        main = [{"path": "migrations/202610290003_batch_chapter_purchases.sql", "sha": blob}]
        pulls = [
            {"number": 400, "files": [
                {"filename": "migrations/202611010042_replayed_batch_purchase.sql",
                 "sha": blob, "status": "added"},
                {"filename": "migrations/202611010043_removed.sql",
                 "sha": blob, "status": "removed"},
            ]},
            {"number": 401, "files": [
                {"filename": "migrations/202611010042_replayed_batch_purchase.sql",
                 "sha": blob, "status": "added"},
            ]},
        ]
        results = scan_cross_version_identical_sql(main, pulls)
        self.assertEqual(len(results), 1)
        self.assertEqual(results[0]["blob_sha"], blob)
        self.assertEqual(
            [item["version"] for item in results[0]["versions"]],
            ["202610290003", "202611010042"],
        )
        self.assertEqual(
            results[0]["versions"][1]["files"][0]["owners"],
            ["#400", "#401"],
        )
        self.assertEqual(len(results[0]["versions"][1]["files"]), 1)

    def test_cross_version_scan_does_not_conflate_distinct_sql(self):
        main = [{"path": "migrations/202610290003_batch.sql", "sha": "a" * 40}]
        pulls = [{"number": 402, "files": [
            {"filename": "migrations/202611010042_different.sql",
             "sha": "b" * 40, "status": "added"},
        ]}]
        self.assertEqual(scan_cross_version_identical_sql(main, pulls), [])
        # Same-version clashes are handled by the existing numeric collision scan.
        pulls[0]["files"][0]["filename"] = "migrations/202610290003_conflict.sql"
        pulls[0]["files"][0]["sha"] = "a" * 40
        self.assertEqual(scan_cross_version_identical_sql(main, pulls), [])
        pulls[0]["files"][0]["sha"] = "BAD"
        with self.assertRaises(AuditError):
            scan_cross_version_identical_sql(main, pulls)

    def test_fail_on_conflict_checks_cross_version_identical_blobs(self):
        report = {
            "collisions": [],
            "cross_version_identical_sql": [{"blob_sha": "a" * 40}],
        }
        with patch("audit_sqlx_history.scan_live", return_value=report):
            with patch("sys.stdout", new_callable=io.StringIO):
                self.assertEqual(
                    audit_main(["scan-open-prs", "--fail-on-conflict"]), 2
                )
                self.assertEqual(audit_main(["scan-open-prs"]), 0)


if __name__ == "__main__":
    unittest.main()
