"""Reject silent omissions in the permanent TOR required-check job."""
import json
from pathlib import Path
import sys
import unittest
from functools import partial

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from ci_test_map import SUITE_JOB_IDS
from verify_ci_gate import FOUNDATION_MARKERS, GateError, verify as verify_gate, verify_foundation_log

SHA = "a" * 40
verify = partial(verify_gate, sha=SHA)


def evidence(selected=None, selector_result="success"):
    if selected is None:
        selected = set(SUITE_JOB_IDS)
    flags = {suite: suite in selected for suite in SUITE_JOB_IDS}
    jobs = {
        job_id: {"result": "success" if flags[suite] else "skipped"}
        for suite, job_id in SUITE_JOB_IDS.items()
    }
    jobs["preflight"] = {"result": "success"}
    jobs["select"] = {
        "result": selector_result, "outputs": {"suites": json.dumps(flags)},
    }
    for job_id in FOUNDATION_MARKERS:
        if jobs[job_id]["result"] == "success":
            jobs[job_id]["outputs"] = {"foundation_evidence": f"{SHA}:{job_id}"}
    return jobs


class CIRequiredGateTests(unittest.TestCase):
    def test_main_and_manual_require_complete_pack(self):
        for event in ("push", "workflow_dispatch", "schedule"):
            with self.subTest(event=event):
                report = verify(event, evidence())
                self.assertEqual(report["verified"], len(SUITE_JOB_IDS))
                self.assertEqual(report["foundation_jobs"], sorted(FOUNDATION_MARKERS))
                with self.assertRaises(GateError):
                    verify(event, evidence({"rust"}))

    def test_pr_selection_can_skip_only_unselected_suites(self):
        partial = evidence({"rust", "postgres"})
        self.assertEqual(verify("pull_request", partial)["selected"], 2)
        self.assertEqual(verify("pull_request", evidence(set()))["verified"], 0)
        partial[SUITE_JOB_IDS["postgres"]]["result"] = "skipped"
        with self.assertRaises(GateError):
            verify("pull_request", partial)
        browser = evidence({"rec_browser"})
        browser[SUITE_JOB_IDS["rec_browser"]]["result"] = "skipped"
        with self.assertRaises(GateError):
            verify("pull_request", browser)

    def test_preflight_is_mandatory_even_for_docs_only(self):
        for state in ("failure", "skipped", "cancelled", None):
            with self.subTest(state=state):
                data = evidence(set())
                data["preflight"]["result"] = state
                with self.assertRaises(GateError):
                    verify("pull_request", data)
        data = evidence({"rust"})
        data.pop("preflight")
        with self.assertRaises(GateError):
            verify("pull_request", data)
        data = evidence({"rust"})
        data["preflight"] = None
        with self.assertRaises(GateError):
            verify("pull_request", data)

    def test_failed_selector_runs_full_fallback_pack(self):
        full = evidence(selector_result="failure")
        self.assertTrue(verify("pull_request", full)["fallback_full"])
        full[SUITE_JOB_IDS["postgres"]]["result"] = "skipped"
        with self.assertRaises(GateError):
            verify("pull_request", full)
        with self.assertRaises(GateError):
            verify("push", evidence(selector_result="failure"))

    def test_invalid_dependency_or_unselected_failure_blocks(self):
        for action in ("missing", "extra", "nonobject", "failed_unselected"):
            with self.subTest(action=action):
                data = evidence(set())
                if action == "missing":
                    data.pop(SUITE_JOB_IDS["rust"])
                elif action == "extra":
                    data["other"] = {"result": "success"}
                elif action == "nonobject":
                    data[SUITE_JOB_IDS["rust"]] = None
                else:
                    data[SUITE_JOB_IDS["rust"]]["result"] = "failure"
                with self.assertRaises(GateError):
                    verify("pull_request", data)

    def test_bad_selector_json_or_types_blocks(self):
        invalid = ('{"rust":true}', '{"rust":"false"}', "bad json", "",
                   json.dumps({suite: 1 for suite in SUITE_JOB_IDS}))
        for output in invalid:
            with self.subTest(output=output):
                data = evidence({"rust"})
                data["select"]["outputs"]["suites"] = output
                with self.assertRaises(GateError):
                    verify("pull_request", data)

    def test_cancelled_selector_and_unknown_event_block(self):
        with self.assertRaises(GateError):
            verify("pull_request", evidence(selector_result="cancelled"))
        with self.assertRaises(GateError):
            verify("merge_group", evidence())

    def test_green_foundation_job_needs_evidence_from_its_own_checkout(self):
        for job in FOUNDATION_MARKERS:
            invalid = (None, "", f"{'b' * 40}:{job}", f"{SHA}:other-job")
            for receipt in invalid:
                with self.subTest(job=job, receipt=receipt):
                    data = evidence()
                    data[job]["outputs"] = {"foundation_evidence": receipt}
                    with self.assertRaisesRegex(GateError, f"{job}: missing or mismatched"):
                        verify("push", data)

    def test_selected_pr_checks_only_foundation_jobs_that_ran(self):
        data = evidence({"backup_recovery"})
        self.assertEqual(verify("pull_request", data)["foundation_jobs"], ["sealed-remote-copy"])
        data["sealed-remote-copy"].pop("outputs")
        with self.assertRaisesRegex(GateError, "sealed-remote-copy: missing or mismatched"):
            verify("pull_request", data)

    def test_fallback_main_checks_foundation_outputs_even_on_green_jobs(self):
        data = evidence(selector_result="failure")
        data["purchased-v1-recovery"]["outputs"] = {}
        with self.assertRaisesRegex(GateError, "purchased-v1-recovery: missing or mismatched"):
            verify("pull_request", data)


class FoundationLogTests(unittest.TestCase):
    def test_catalog_requires_each_scenario_and_exact_counts(self):
        job = "paid-private-e2e"
        lines = [
            "FND03_SUPPORTED_UPLOADS_OK formats=4",
            "FND07_MULTI_BOOK_IMPORT_OK books=10 chapters=30 versions=30 objects=30 missing=0 unexpected=0",
            "FND03_CONCURRENT_CATALOG_OK books=100 editions=100 objects=100 workers=2",
            "DROP04D_LOAD_PASS chapters=100000 batches=200 seconds=387.471",
            "DROP04D_WORKER_RETRY_PASS: reclaimed stage, exact PACK, no SINGLE, atomic job",
            "DROP07D_PG_MINIO_OPERATOR_PASS: bounded LIST+PG classification across two pages, fenced checkpoint, unknown never GC",
        ]
        self.assertEqual(verify_foundation_log(job, lines, SHA), f"{SHA}:{job}")
        for index, marker in enumerate(lines):
            with self.subTest(marker=marker):
                invalid = lines[:index] + [f"warning: {marker}"] + lines[index + 1:]
                with self.assertRaisesRegex(GateError, "missing foundation evidence"):
                    verify_foundation_log(job, invalid, SHA)
        lines[1] = lines[1].replace("missing=0", "missing=1")
        with self.assertRaisesRegex(GateError, "ten books with exact inventory"):
            verify_foundation_log(job, lines, SHA)

    def test_recovery_requires_pack_rights_wal_and_independent_paid_proof(self):
        job = "sealed-remote-copy"
        log = [
            "I11_PRIVATE_S3_ENCRYPTED_BACKUP_VERIFIED",
            "DROP07_RESTORED_PACK_HISTORICAL_V1_OK",
            "FND05_RESTORED_FREE_ARCHIVE_HTTP_OK",
            "DEPLOY07C_SAVED_FREE_LAYER_AND_REVOKED_DENIAL_OK",
            "DEPLOY07C_CORRUPT_RESTORED_PACK_DENIED",
            "I11_RESTORED_CHECKOUT_FINANCE_OK",
            "I11_RESTORED_REFUND_FINANCE_REVOCATION_OK",
            "I11_RESTORED_HISTORICAL_RIGHTS_DB_OK",
            "D001_OFFSITE_VERIFIED_BEFORE_FIXTURE_SEAL_OK",
            "D001_OFFSITE_RESTORED_SEAL_AND_V2_OK",
            "D001_OFFSITE_SINGLE_EXACT_RESTORE_CI_OK",
            "FND05_RESTORED_ACTIVE_COVER_HTTP_OK",
            "I11_PITR_ISOLATED_DRILL_OK report=/runner/drill.json",
        ]
        self.assertEqual(verify_foundation_log(job, log, SHA), f"{SHA}:{job}")
        with self.assertRaisesRegex(GateError, "WAL recovery"):
            verify_foundation_log(job, log[:-1], SHA)
        paid = "purchased-v1-recovery"
        with self.assertRaisesRegex(GateError, "purchased V1 and KEK"):
            verify_foundation_log(paid, log, SHA)
        paid_log = [
            "S334_PURCHASED_V1_AFTER_V2_PG_S3_KEK_RESTORE_PASS",
            "DEPLOY07C_PAID_V1_V2_DB_EXACT_OK",
            "DEPLOY07C_MISSING_WRONG_KEYS_AND_PROVIDER_DENIED",
            "DEPLOY07C_CROSS_PURCHASE_AND_ANONYMOUS_DENIED",
            "DEPLOY07C_BOTH_RESTORED_REFUNDS_DENIED",
            "DEPLOY07C_PAID_SOURCE_UNCHANGED_OK",
            "S334_HISTORICAL_KEK_V1_V2_RESTORE_PASS",
        ]
        self.assertEqual(verify_foundation_log(paid, paid_log, SHA), f"{SHA}:{paid}")
        for job_id, complete in ((job, log), (paid, paid_log)):
            for index, marker in enumerate(complete):
                with self.subTest(job=job_id, marker=marker):
                    with self.assertRaisesRegex(GateError, "missing foundation evidence"):
                        verify_foundation_log(job_id, complete[:index] + complete[index+1:], SHA)
                    for altered in (f"warning: {marker}", f"{marker} (skipped)"):
                        with self.assertRaisesRegex(GateError, "missing foundation evidence"):
                            verify_foundation_log(
                                job_id, complete[:index] + [altered] + complete[index+1:], SHA
                            )


if __name__ == "__main__":
    unittest.main()
