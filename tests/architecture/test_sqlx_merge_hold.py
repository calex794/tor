"""Offline tests for the SHA-pinned, read-only migration HOLD gate."""
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
HEAD = "a" * 40
BASE = "b" * 40
NEW_HEAD = "c" * 40
NEW_BASE = "d" * 40


def pr(head=HEAD, base=BASE, state="open", number=400):
    return {"number": number, "state": state,
            "head": {"sha": head}, "base": {"sha": base}}


def documents():
    return [{"filename": "docs/design.md", "status": "modified"}]


def migration():
    return [{"filename": "migrations/202610310294_feature.sql", "status": "added"}]


class SqlxMergeHoldTests(unittest.TestCase):
    def check(self, responses, head=HEAD, base=BASE):
        with patch("check_sqlx_merge_hold.github_json", side_effect=responses) as getter:
            return enforce_hold(REPO, 400, TOKEN, head, base), getter.call_count

    def test_docs_only_passes_with_stable_head_base(self):
        message, count = self.check([pr(), documents(), pr()])
        self.assertIn("no changed SQLx", message)
        self.assertEqual(count, 3)

    def test_migration_blocks_if_hold_open(self):
        with self.assertRaisesRegex(HoldCheckError, "HOLD #235"):
            self.check([pr(), migration(), {"state": "open"}, pr()])

    def test_merged_main_duplicate_scenario_blocks(self):
        renamed = [{"filename": "migrations/202610310293_d06_refund_admin_step_up.sql",
                    "previous_filename": "migrations/202610310292_d06_refund_admin_step_up.sql",
                    "status": "renamed"}]
        with self.assertRaises(HoldCheckError):
            self.check([pr(), renamed, {"state": "open"}, pr()])

    def test_completed_hold_is_not_a_db_approval(self):
        message, _ = self.check([
            pr(), migration(), {"state": "closed", "state_reason": "completed"}, pr()])
        self.assertIn("still requires private", message)

    def test_closed_not_planned_still_blocks(self):
        with self.assertRaisesRegex(HoldCheckError, "not signed off"):
            self.check([pr(), migration(),
                        {"state": "closed", "state_reason": "not_planned"}, pr()])

    def test_changed_head_during_docs_only_scan_fails(self):
        with self.assertRaisesRegex(HoldCheckError, "changed during"):
            self.check([pr(), documents(), pr(head=NEW_HEAD)])

    def test_changed_base_during_migration_scan_fails(self):
        with self.assertRaisesRegex(HoldCheckError, "changed during"):
            self.check([pr(), migration(),
                        {"state": "closed", "state_reason": "completed"},
                        pr(base=NEW_BASE)])

    def test_stale_event_head_or_base_fails_before_reading_files(self):
        for actual in (pr(head=NEW_HEAD), pr(base=NEW_BASE)):
            with self.subTest(actual=actual):
                with patch("check_sqlx_merge_hold.github_json", return_value=actual) as get:
                    with self.assertRaisesRegex(HoldCheckError, "differs from the CI event"):
                        enforce_hold(REPO, 400, TOKEN, HEAD, BASE)
                    self.assertEqual(get.call_count, 1)

    def test_closed_or_invalid_pr_identity_fails_closed(self):
        for response in (pr(state="closed"), pr(number=401),
                         {"number": 400, "state": "open", "head": {}},
                         {"number": 400, "state": "open", "head": {"sha": "x"},
                          "base": {"sha": BASE}}):
            with self.subTest(response=response):
                with patch("check_sqlx_merge_hold.github_json", return_value=response):
                    with self.assertRaises(HoldCheckError):
                        enforce_hold(REPO, 400, TOKEN, HEAD, BASE)

    def test_invalid_expected_sha_and_missing_token_fail(self):
        for head, base in ((None, BASE), (HEAD, None), ("xyz", BASE),
                           (HEAD, "A" * 40)):
            with self.subTest(head=head, base=base):
                with patch("check_sqlx_merge_hold.github_json") as get:
                    with self.assertRaisesRegex(HoldCheckError, "Exact pull request"):
                        enforce_hold(REPO, 400, TOKEN, head, base)
                    get.assert_not_called()
        with self.assertRaisesRegex(HoldCheckError, "token required"):
            enforce_hold(REPO, 400, "", HEAD, BASE)

    def test_rename_remove_and_non_sql(self):
        self.assertTrue(changes_sqlx([{"filename": "elsewhere/file",
                          "previous_filename": "migrations/202610310293_old.sql"}]))
        self.assertTrue(changes_sqlx([{"filename": "migrations/202610310293_old.sql",
                                      "status": "removed"}]))
        self.assertFalse(changes_sqlx([{"filename": "migrations/README.md"}]))

    def test_complete_pagination(self):
        first = [{"filename": f"docs/{n}.md"} for n in range(100)]
        with patch("check_sqlx_merge_hold.github_json",
                   side_effect=[first, migration()]) as get:
            files = changed_pr_files(REPO, 400, TOKEN)
        self.assertEqual(len(files), 101)
        self.assertTrue(changes_sqlx(files))
        self.assertIn("page=1", get.call_args_list[0].args[0])
        self.assertIn("page=2", get.call_args_list[1].args[0])

    def test_invalid_file_listing_and_repo_rejected(self):
        for repo, number in (("../unsafe", 400), (REPO, -3),
                             (REPO, True), ("../tor", 400)):
            with self.subTest(repo=repo, number=number):
                with self.assertRaises(HoldCheckError):
                    changed_pr_files(repo, number, TOKEN)
        with patch("check_sqlx_merge_hold.github_json", return_value=[]):
            with self.assertRaisesRegex(HoldCheckError, "Empty PR"):
                changed_pr_files(REPO, 400, TOKEN)
        with patch("check_sqlx_merge_hold.github_json", return_value=[{}]):
            with self.assertRaisesRegex(HoldCheckError, "Invalid PR"):
                changed_pr_files(REPO, 400, TOKEN)

    def test_cli_fails_without_leaking_token(self):
        env = {"GITHUB_REPOSITORY": REPO, "TOR_PULL_REQUEST_NUMBER": "400",
               "TOR_PULL_REQUEST_HEAD_SHA": HEAD, "TOR_PULL_REQUEST_BASE_SHA": BASE,
               "GITHUB_TOKEN": TOKEN}
        with patch.dict("os.environ", env, clear=True):
            with patch("check_sqlx_merge_hold.github_json",
                       side_effect=[pr(), migration(), {"state": "open"}, pr()]):
                stderr = io.StringIO()
                with contextlib.redirect_stderr(stderr):
                    self.assertEqual(main([]), 2)
                self.assertIn("TOR_CI_REFUSED", stderr.getvalue())
                self.assertNotIn(TOKEN, stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
