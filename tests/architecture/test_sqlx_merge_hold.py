"""Offline, read-only tests for the migration-bearing PR HOLD gate."""
import contextlib
import io
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_sqlx_merge_hold import (
    HoldCheckError, changed_pr_files, changes_sqlx, enforce_hold, main,
)

REPO = "calex794/tor"
TOKEN = "mock-non-secret-token"


class SqlxMergeHoldTests(unittest.TestCase):
    def test_docs_only_pr_is_allowed_without_issue_lookup(self):
        with patch("check_sqlx_merge_hold.github_json") as getter:
            getter.return_value = [
                {"filename": "migrations/README.md", "status": "modified"},
                {"filename": "docs/contracts/notes.md", "status": "added"},
            ]
            self.assertIn("no changed SQLx", enforce_hold(REPO, 373, TOKEN))
            self.assertEqual(getter.call_count, 1)

    def test_new_migration_blocks_when_hold_issue_open(self):
        with patch("check_sqlx_merge_hold.github_json") as getter:
            getter.side_effect = [
                [{"filename": "migrations/202610310294_something.sql", "status": "added"}],
                {"state": "open", "state_reason": None},
            ]
            with self.assertRaisesRegex(HoldCheckError, "HOLD #235"):
                enforce_hold(REPO, 365, TOKEN)
            self.assertEqual(getter.call_count, 2)

    def test_merged_main_duplicate_scenario_is_blocked_before_sqlx_merge(self):
        # Even if the proposed tree has no numeric duplicate *yet*, issue #235
        # blocks a new migration until the retained-ledger owners sign off.
        with patch("check_sqlx_merge_hold.github_json") as getter:
            getter.side_effect = [
                [{"filename": "migrations/202610310293_d06_refund_admin_step_up.sql",
                  "status": "renamed",
                  "previous_filename": "migrations/202610310292_d06_refund_admin_step_up.sql"}],
                {"state": "open", "state_reason": None},
            ]
            with self.assertRaises(HoldCheckError):
                enforce_hold(REPO, 365, TOKEN)

    def test_completed_hold_issue_allows_policy_gate_but_not_db_approval(self):
        with patch("check_sqlx_merge_hold.github_json") as getter:
            getter.side_effect = [
                [{"filename": "migrations/202610310292_book_engagement_integrity.sql",
                  "status": "added"}],
                {"state": "closed", "state_reason": "completed"},
            ]
            self.assertIn("still requires private", enforce_hold(REPO, 364, TOKEN))

    def test_closed_not_planned_is_not_equivalent_to_signed_off(self):
        with patch("check_sqlx_merge_hold.github_json") as getter:
            getter.side_effect = [
                [{"filename": "migrations/202610310294_feature.sql"}],
                {"state": "closed", "state_reason": "not_planned"},
            ]
            with self.assertRaisesRegex(HoldCheckError, "not signed off"):
                enforce_hold(REPO, 400, TOKEN)

    def test_rename_or_removed_migration_counts_as_sqlx_change(self):
        self.assertTrue(changes_sqlx([
            {"filename": "other/newname.txt",
             "previous_filename": "migrations/202610310293_deleted.sql",
             "status": "renamed"}
        ]))
        self.assertTrue(changes_sqlx([
            {"filename": "migrations/202610310293_removed.sql", "status": "removed"}
        ]))
        self.assertFalse(changes_sqlx([
            {"filename": "migrations/README.md", "status": "modified"}
        ]))

    def test_pr_files_pagination_is_complete_before_policy_decision(self):
        first = [{"filename": f"docs/d{i}.md"} for i in range(100)]
        last = [{"filename": "migrations/202610310294_feature.sql"}]
        with patch("check_sqlx_merge_hold.github_json", side_effect=[
            first, last
        ]) as getter:
            files = changed_pr_files(REPO, 400, TOKEN)
        self.assertEqual(len(files), 101)
        self.assertTrue(changes_sqlx(files))
        self.assertIn("page=1", getter.call_args_list[0].args[0])
        self.assertIn("page=2", getter.call_args_list[1].args[0])

    def test_missing_token_pr_and_api_failure_fail_closed(self):
        with self.assertRaisesRegex(HoldCheckError, "token required"):
            enforce_hold(REPO, 400, "")
        for repo, pr in [("../unsafe", 400), (REPO, -3), (REPO, True)]:
            with self.subTest(repo=repo, pr=pr):
                with self.assertRaises(HoldCheckError):
                    changed_pr_files(repo, pr, TOKEN)
        with patch("check_sqlx_merge_hold.github_json", return_value=[]):
            with self.assertRaisesRegex(HoldCheckError, "Empty PR file"):
                enforce_hold(REPO, 400, TOKEN)
        with patch("check_sqlx_merge_hold.github_json",
                   return_value=[{"status": "modified"}]):
            with self.assertRaisesRegex(HoldCheckError, "Invalid PR"):
                enforce_hold(REPO, 400, TOKEN)

    def test_cli_returns_blocked_without_credential_output(self):
        env = {
            "GITHUB_REPOSITORY": REPO,
            "TOR_PULL_REQUEST_NUMBER": "365",
            "GITHUB_TOKEN": TOKEN,
        }
        with patch.dict("os.environ", env, clear=True):
            with patch("check_sqlx_merge_hold.github_json", side_effect=[
                [{"filename": "migrations/202610310293_refund.sql"}],
                {"state": "open"},
            ]):
                stderr = io.StringIO()
                with contextlib.redirect_stderr(stderr):
                    self.assertEqual(main([]), 2)
                self.assertIn("TOR_CI_REFUSED", stderr.getvalue())
                self.assertNotIn(TOKEN, stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
