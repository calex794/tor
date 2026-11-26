"""Forward-only deployment: main advancement is safe; rollback is not."""
from pathlib import Path
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import release_ancestry as guard

ACTIVE, CANDIDATE, HEAD, NEW_HEAD = (letter * 40 for letter in "abcd")


class History:
    def __init__(self, head=HEAD, comparison=None):
        self.head = head
        self.comparison = comparison
        self.calls = []

    def get(self, path, params=None):
        self.calls.append((path, params))
        if path.endswith("/git/ref/heads/main"):
            return {"ref": "refs/heads/main",
                    "object": {"type": "commit", "sha": self.head}}
        base, descendant = path.rsplit("/", 1)[1].split("...")
        if params != {"per_page": 1}:
            raise AssertionError("comparison must be bounded")
        ordered = (ACTIVE, CANDIDATE, HEAD, NEW_HEAD)
        status = "ahead" if ordered.index(base) < ordered.index(descendant) else "behind"
        result = {"status": status, "base_commit": {"sha": base},
                  "merge_base_commit": {"sha": base if status == "ahead" else descendant},
                  "ahead_by": 1 if status == "ahead" else 0,
                  "behind_by": 0 if status == "ahead" else 1}
        if self.comparison:
            result.update(self.comparison)
        return result


class ForwardReleaseTests(unittest.TestCase):
    def test_main_can_advance_between_packaging_and_activation(self):
        for head in (CANDIDATE, HEAD, NEW_HEAD):
            history = History(head)
            self.assertEqual(guard.verify_forward(history.get, CANDIDATE, ACTIVE), head)
            self.assertIn((f"/repos/{guard.REPO}/compare/{ACTIVE}...{CANDIDATE}",
                           {"per_page": 1}), history.calls)

    def test_replay_behind_active_is_refused_even_on_main(self):
        with self.assertRaises(guard.AncestryError):
            guard.verify_forward(History().get, ACTIVE, CANDIDATE)

    def test_candidate_ahead_of_main_is_refused(self):
        with self.assertRaises(guard.AncestryError):
            guard.verify_forward(History(HEAD).get, NEW_HEAD, ACTIVE)

    def test_same_release_is_idempotent(self):
        self.assertEqual(guard.verify_forward(History().get, CANDIDATE, CANDIDATE), HEAD)

    def test_divergence_and_invalid_comparison_fail_closed(self):
        for override in ({"status": "diverged"}, {"behind_by": 1},
                         {"ahead_by": 0}, {"ahead_by": True},
                         {"base_commit": {"sha": HEAD}},
                         {"merge_base_commit": {"sha": HEAD}}, {"base_commit": None}):
            with self.subTest(override=override), self.assertRaises(guard.AncestryError):
                guard.verify_forward(History(comparison=override).get, CANDIDATE, ACTIVE)

    def test_bad_ref_or_network_never_authorizes_release(self):
        for response in ({}, {"ref": "refs/heads/main",
                              "object": {"type": "tag", "sha": HEAD}},
                         {"ref": "refs/heads/feature",
                          "object": {"type": "commit", "sha": HEAD}}):
            with self.subTest(response=response), self.assertRaises(guard.AncestryError):
                guard.verify_forward(lambda *_: response, CANDIDATE, ACTIVE)
        with self.assertRaises(OSError):
            guard.verify_forward(mock.Mock(side_effect=OSError("offline")), CANDIDATE, ACTIVE)
        with mock.patch.object(guard, "active_sha", return_value=ACTIVE), \
             mock.patch.object(guard, "public_get", side_effect=OSError("offline")), \
             mock.patch.object(sys, "argv", ["guard", CANDIDATE, "--from-active"]):
            self.assertEqual(guard.main(), 1)

    def test_untrusted_active_or_candidate_sha_never_compared(self):
        with self.assertRaises(guard.AncestryError):
            guard.verify_forward(History().get, "abc", ACTIVE)
        with self.assertRaises(guard.AncestryError):
            guard.verify_forward(History().get, CANDIDATE, "not-a-sha")

    def test_root_guard_under_lock_and_rechecked_before_migrations_and_cutover(self):
        helper = (ROOT / "ops/one-click-release-vps.sh").read_text()
        go_live = (ROOT / "ops/go-live-postgres-vps.sh").read_text()
        bootstrap = (ROOT / "ops/bootstrap-one-click-release-vps.sh").read_text()
        self.assertLess(helper.index("flock -n 9"),
                        helper.index('python3 -I "$guard" "$sha" --from-active'))
        self.assertLess(helper.index('python3 -I "$guard" "$sha" --from-active'),
                        helper.index('bash "$runner" "$sha" --forward'))
        self.assertIn('bash "$runner" "$sha" --recover --forward', helper)
        self.assertIn('python3 -I "$guard" "$sha" --from-active', go_live)
        self.assertEqual(go_live.count("\nrequire_current_main_sha\n"), 2)
        self.assertLess(bootstrap.index('python3 -I "$guard_download" "$sha" --main-only'),
                        bootstrap.index('install -o root -g root -m 0755 "$guard_download"'))


if __name__ == "__main__":
    unittest.main()
