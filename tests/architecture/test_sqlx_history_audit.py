"""Read-only SQLx history and parallel PR collision regression tests."""
import csv
import hashlib
import io
import stat
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from audit_sqlx_history import (
    AuditError, evaluate_history, local_migrations, parse_inventory,
    read_from_psql, scan_conflicts, scan_cross_version_identical_sql, scan_live,
    focus_report_on_pr, main as audit_main,
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

    def test_pending_lower_number_blocks_even_without_applied_drift(self):
        (self.root / "202610290002_layer_access.sql").write_text("SELECT 1;\n")
        current = local_migrations(self.root)
        name, installed = parse_inventory(inventory([self.valid]))
        result = evaluate_history(current, name, installed)
        self.assertEqual(result["pending"], ["202610290002"])
        self.assertEqual(result["out_of_order_pending"], ["202610290002"])
        self.assertEqual(result["matched"], [self.version])
        self.assertEqual(result["status"], "BLOCK")
        self.assertTrue(any(
            "older than latest applied version" in blocker
            and "202610290002" in blocker
            for blocker in result["blockers"]
        ))

    def test_newer_pending_migration_is_not_applied_drift(self):
        (self.root / "202611010020_future_table.sql").write_text("SELECT 42;\n")
        current = local_migrations(self.root)
        name, installed = parse_inventory(inventory([self.valid]))
        result = evaluate_history(current, name, installed)
        self.assertEqual(result["pending"], ["202611010020"])
        self.assertEqual(result["out_of_order_pending"], [])
        self.assertEqual(result["status"], "NO_APPLIED_DRIFT_DETECTED")
        self.assertEqual(result["blockers"], [])

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

    def test_same_sql_content_different_local_versions_fails_closed(self):
        duplicate = self.root / "202611010009_duplicate_purchase_table.sql"
        duplicate.write_bytes(self.sql)
        with self.assertRaisesRegex(AuditError, "Identical SQLx SQL bytes") as error:
            local_migrations(self.root)
        self.assertIn(self.version, str(error.exception))
        self.assertIn("202611010009", str(error.exception))

    def test_identical_applied_checksum_under_different_versions_blocks(self):
        other_version = "202611010009"
        digest = self.valid["checksum_sha384_hex"]
        # The supplied local mapping is constructed directly to prove that the
        # applied ledger is validated even if local_migrations() was bypassed.
        expected = {
            **self.expected,
            other_version: {
                "filename": other_version + "_duplicate_purchase_table.sql",
                "description": "duplicate purchase table",
                "checksum": digest,
            },
        }
        same_sql = record(
            other_version, "duplicate purchase table", digest.upper()
        )
        database, applied = parse_inventory(inventory([self.valid, same_sql]))
        result = evaluate_history(expected, database, applied)
        self.assertEqual(result["matched"], [self.version, other_version])
        self.assertEqual(result["status"], "BLOCK")
        self.assertTrue(any(
            "Identical applied SQLx checksum" in blocker
            and self.version in blocker and other_version in blocker
            for blocker in result["blockers"]
        ))

    def test_distinct_applied_checksums_are_not_false_duplicates(self):
        other_version = "202611010010"
        other_sql = b"CREATE TABLE receipts(id bigint);\n"
        checksum = hashlib.sha384(other_sql).hexdigest()
        expected = {
            **self.expected,
            other_version: {
                "filename": other_version + "_receipts.sql",
                "description": "receipts",
                "checksum": checksum,
            },
        }
        other_row = record(other_version, "receipts", checksum)
        database, applied = parse_inventory(inventory([self.valid, other_row]))
        result = evaluate_history(expected, database, applied)
        self.assertEqual(result["status"], "NO_APPLIED_DRIFT_DETECTED")
        self.assertEqual(result["blockers"], [])

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

    def test_private_psql_export_preserves_full_ledger_and_refuses_overwrite(self):
        private = self.root / "private"
        private.mkdir(mode=0o700)
        output = private / "ledger.csv"
        raw = inventory([self.valid])
        command = [
            "inventory", "--from-psql", "--migrations", str(self.root),
            "--save-raw-csv", str(output),
        ]
        with patch("audit_sqlx_history.read_from_psql", return_value=raw):
            with patch("sys.stdout", new_callable=io.StringIO) as stdout:
                self.assertEqual(audit_main(command), 0)
        self.assertEqual(output.read_bytes().decode("utf-8"), raw)
        self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o600)
        self.assertNotIn(self.valid["checksum_sha384_hex"], stdout.getvalue())

        # Existing audit evidence must not be silently replaced or appended.
        with patch("audit_sqlx_history.read_from_psql", return_value=raw):
            with patch("sys.stderr", new_callable=io.StringIO):
                with patch("sys.stdout", new_callable=io.StringIO):
                    self.assertEqual(audit_main(command), 2)
        self.assertEqual(output.read_bytes().decode("utf-8"), raw)

    def test_private_export_rejects_world_readable_directory_and_symlink(self):
        private = self.root / "private"
        private.mkdir(mode=0o700)
        shared = self.root / "shared"
        shared.mkdir(mode=0o755)
        shared.chmod(0o755)  # An operator's umask must not weaken this test.
        sensitive = private / "original.csv"
        sensitive.write_text("do not touch", encoding="utf-8")
        (private / "alias.csv").symlink_to(sensitive)
        raw = inventory([self.valid])
        for target in [shared / "ledger.csv", private / "alias.csv"]:
            with self.subTest(target=target):
                with patch("audit_sqlx_history.read_from_psql", return_value=raw):
                    with patch("sys.stderr", new_callable=io.StringIO):
                        with patch("sys.stdout", new_callable=io.StringIO):
                            self.assertEqual(audit_main([
                                "inventory", "--from-psql", "--migrations",
                                str(self.root), "--save-raw-csv", str(target)
                            ]), 2)
        self.assertFalse((shared / "ledger.csv").exists())
        self.assertEqual(sensitive.read_text(encoding="utf-8"), "do not touch")

    def test_private_export_requires_valid_psql_inventory_not_csv_replay(self):
        private = self.root / "private"
        private.mkdir(mode=0o700)
        output = private / "ledger.csv"
        with patch("audit_sqlx_history.read_from_psql", return_value="broken"):
            with patch("sys.stderr", new_callable=io.StringIO):
                self.assertEqual(audit_main([
                    "inventory", "--from-psql", "--migrations", str(self.root),
                    "--save-raw-csv", str(output),
                ]), 2)
        self.assertFalse(output.exists())
        source = private / "previous.csv"
        source.write_text(inventory([self.valid]), encoding="utf-8")
        with patch("sys.stderr", new_callable=io.StringIO):
            self.assertEqual(audit_main([
                "inventory", "--csv", str(source), "--migrations",
                str(self.root), "--save-raw-csv", str(output),
            ]), 2)
        self.assertFalse(output.exists())

    def test_private_export_persists_blocking_ledger_for_owner_review(self):
        private = self.root / "private"
        private.mkdir(mode=0o700)
        output = private / "blocked.csv"
        bad = inventory([dict(self.valid, checksum_sha384_hex="a" * 96)])
        with patch("audit_sqlx_history.read_from_psql", return_value=bad):
            with patch("sys.stdout", new_callable=io.StringIO) as stdout:
                self.assertEqual(audit_main([
                    "inventory", "--from-psql", "--migrations", str(self.root),
                    "--save-raw-csv", str(output),
                ]), 2)
        self.assertEqual(output.read_bytes().decode("utf-8"), bad)
        self.assertIn('"status": "BLOCK"', stdout.getvalue())

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


    def test_focus_one_pr_preserves_global_evidence_and_excludes_other_conflicts(self):
        report = {
            "open_pr_numbers": [348, 364, 365, 400],
            "collisions": [
                {"version": "202610310292", "variants": [
                    {"owners": ["#364"]}, {"owners": ["#401"]},
                ]},
                {"version": "202610310293", "variants": [
                    {"owners": ["#348"]}, {"owners": ["#365"]},
                ]},
            ],
            "cross_version_identical_sql": [
                {"blob_sha": "a" * 40, "versions": [
                    {"files": [{"owners": ["#400"]}]},
                    {"files": [{"owners": ["main"]}]},
                ]},
            ],
            "notice": "Read-only snapshot.",
        }
        result = focus_report_on_pr(report, 365)
        self.assertEqual(result["focus_pr"], 365)
        self.assertEqual(result["global_numeric_collisions"], 2)
        self.assertEqual(result["global_cross_version_reuses"], 1)
        self.assertEqual([c["version"] for c in result["collisions"]],
                         ["202610310293"])
        self.assertEqual(result["cross_version_identical_sql"], [])
        self.assertEqual(len(report["collisions"]), 2)
        self.assertEqual(len(report["cross_version_identical_sql"]), 1)
        self.assertIn("not a reservation", result["notice"])

        with patch("audit_sqlx_history.scan_live", return_value=report):
            with patch("sys.stdout", new_callable=io.StringIO) as output:
                self.assertEqual(audit_main([
                    "scan-open-prs", "--focus-pr", "365", "--fail-on-conflict"
                ]), 2)
                self.assertIn('"202610310293"', output.getvalue())
                self.assertNotIn('"202610310292"', output.getvalue())

    def test_focus_pr_no_own_conflicts_is_not_blocked_by_other_open_drafts(self):
        report = {
            "open_pr_numbers": [348, 364, 365, 400],
            "collisions": [{"version": "202610310293", "variants": [
                {"owners": ["#348"]}, {"owners": ["#365"]},
            ]}],
            "cross_version_identical_sql": [],
            "notice": "Read-only snapshot.",
        }
        with patch("audit_sqlx_history.scan_live", return_value=report):
            with patch("sys.stdout", new_callable=io.StringIO) as output:
                self.assertEqual(audit_main([
                    "scan-open-prs", "--focus-pr", "400", "--fail-on-conflict"
                ]), 0)
                self.assertIn('"global_numeric_collisions": 1',
                              output.getvalue())
                self.assertIn('"collisions": []', output.getvalue())

    def test_focus_pr_missing_or_invalid_fails_closed(self):
        report = {
            "open_pr_numbers": [348, 365], "collisions": [],
            "cross_version_identical_sql": [], "notice": "Read-only.",
        }
        for pr in [0, -1, 364]:
            with self.subTest(pr=pr):
                with self.assertRaises(AuditError):
                    focus_report_on_pr(report, pr)
                with patch("audit_sqlx_history.scan_live", return_value=report):
                    with patch("sys.stderr", new_callable=io.StringIO):
                        self.assertEqual(audit_main([
                            "scan-open-prs", "--focus-pr", str(pr),
                            "--fail-on-conflict",
                        ]), 2)

    def test_focus_pr_can_detect_exact_cross_version_sql_blob_reuse(self):
        report = {
            "open_pr_numbers": [365], "collisions": [],
            "cross_version_identical_sql": [
                {"blob_sha": "b" * 40, "versions": [
                    {"version": "202610310292",
                     "files": [{"owners": ["main"]}]},
                    {"version": "202610310293",
                     "files": [{"owners": ["#365"]}]},
                ]},
            ],
            "notice": "Read-only.",
        }
        result = focus_report_on_pr(report, 365)
        self.assertEqual(result["collisions"], [])
        self.assertEqual(len(result["cross_version_identical_sql"]), 1)
        with patch("audit_sqlx_history.scan_live", return_value=report):
            with patch("sys.stdout", new_callable=io.StringIO):
                self.assertEqual(audit_main([
                    "scan-open-prs", "--focus-pr", "365", "--fail-on-conflict"
                ]), 2)

    @staticmethod
    def live_pr(number=400, head="a" * 40, base="b" * 40):
        return {"number": number, "head": {"sha": head}, "base": {"sha": base}}

    def mocked_open_pr_scan(self, initial_prs, final_prs, *,
                            initial_main="c" * 40, final_main=None):
        main_versions = iter([initial_main, final_main or initial_main])
        pr_versions = iter([initial_prs, final_prs])

        def fake_json(path, token=None):
            if path.endswith("/branches/main"):
                return {"commit": {"sha": next(main_versions)}}
            if "/contents/migrations?ref=" in path:
                return [{
                    "path": "migrations/202610290003_batch_chapter_purchases.sql",
                    "sha": "1" * 40,
                }]
            self.fail(f"Unexpected GitHub GET path: {path}")

        def fake_pages(path, token=None):
            if path.endswith("/pulls?state=open&per_page=100"):
                return next(pr_versions)
            if "/pulls/" in path and path.endswith("/files?per_page=100"):
                return [{
                    "filename": "migrations/202611010042_other_feature.sql",
                    "sha": "2" * 40, "status": "added",
                }]
            self.fail(f"Unexpected paginated GitHub GET path: {path}")

        with patch("audit_sqlx_history.github_json", side_effect=fake_json):
            with patch("audit_sqlx_history.github_pages", side_effect=fake_pages):
                return scan_live("calex794/tor")

    def test_live_scan_stable_snapshot_succeeds(self):
        prs = [self.live_pr()]
        report = self.mocked_open_pr_scan(prs, prs)
        self.assertEqual(report["main_sha"], "c" * 40)
        self.assertEqual(report["open_prs_scanned"], 1)
        self.assertEqual(report["collisions"], [])
        self.assertEqual(report["cross_version_identical_sql"], [])

    def test_live_scan_moving_pr_head_fails_closed(self):
        before = [self.live_pr()]
        after = [self.live_pr(head="d" * 40)]
        with self.assertRaisesRegex(AuditError, "membership/head/base changed"):
            self.mocked_open_pr_scan(before, after)

    def test_live_scan_moving_pr_base_fails_closed(self):
        before = [self.live_pr()]
        after = [self.live_pr(base="e" * 40)]
        with self.assertRaisesRegex(AuditError, "membership/head/base changed"):
            self.mocked_open_pr_scan(before, after)

    def test_live_scan_added_or_closed_pr_fails_closed(self):
        before = [self.live_pr()]
        for after in [[], [self.live_pr(), self.live_pr(401)]]:
            with self.subTest(after_count=len(after)):
                with self.assertRaisesRegex(AuditError, "membership/head/base changed"):
                    self.mocked_open_pr_scan(before, after)

    def test_live_scan_moving_main_fails_closed(self):
        prs = [self.live_pr()]
        with self.assertRaisesRegex(AuditError, "main moved"):
            self.mocked_open_pr_scan(prs, prs, final_main="e" * 40)

    def test_live_scan_duplicate_paginated_pr_fails_closed(self):
        prs = [self.live_pr(), self.live_pr()]
        with self.assertRaisesRegex(AuditError, "Duplicate open PR"):
            self.mocked_open_pr_scan(prs, prs)


if __name__ == "__main__":
    unittest.main()
