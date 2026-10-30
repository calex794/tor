"""One-button TOR release contracts: manual-only, green CI, safe root boundary."""

from pathlib import Path
import copy
import importlib.util
import sys
from urllib.error import HTTPError
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
WORKFLOW = (ROOT / ".github/workflows/tor-one-click-release.yml").read_text(encoding="utf-8")
HELPER = (ROOT / "ops/one-click-release-vps.sh").read_text(encoding="utf-8")
BOOTSTRAP = (ROOT / "ops/bootstrap-one-click-release-vps.sh").read_text(encoding="utf-8")
GO_LIVE = (ROOT / "ops/go-live-postgres-vps.sh").read_text(encoding="utf-8")
MIGRATE = (ROOT / "ops/apply-tor-migrations-vps.sh").read_text(encoding="utf-8")
spec = importlib.util.spec_from_file_location(
    "one_click_gate", ROOT / "tools/check_one_click_release_ci.py"
)
assert spec and spec.loader
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
SHA = "a" * 40


def protected_main(enabled=True, level="everyone",
                   contexts=("rules", "ci-required")):
    return {
        "name": "main",
        "protected": enabled,
        "protection": {
            "enabled": enabled,
            "required_status_checks": {
                "enforcement_level": level,
                "contexts": list(contexts),
                "checks": [{"context": x, "app_id": 15368} for x in contexts],
            },
        },
    }


def active_main_policy():
    branch = protected_main(enabled=False, level="off", contexts=())
    branch["protected"] = True  # exact real GitHub API combination
    ruleset = {
        "id": 26235453,
        "source": "calex794/tor",
        "source_type": "Repository",
        "target": "branch",
        "enforcement": "active",
        "updated_at": "2026-10-31T00:28:58.370+05:00",
        "current_user_can_bypass": "never",
        # bypass_actors intentionally omitted by GitHub read-only tokens
        "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"],
                                    "exclude": []}},
        "rules": [
            {"type": "deletion"},
            {"type": "non_fast_forward"},
            {"type": "pull_request",
             "parameters": {"required_approving_review_count": 0}},
            {"type": "required_status_checks", "parameters": {
                "strict_required_status_checks_policy": False,
                "do_not_enforce_on_create": False,
                "required_status_checks": [
                    {"context": "rules", "integration_id": 15368},
                    {"context": "ci-required", "integration_id": 15368},
                ],
            }},
        ],
    }
    effective = [
        dict(row, ruleset_id=ruleset["id"]) for row in ruleset["rules"]
    ]
    return branch, [ruleset], effective


def run(name, status="completed", conclusion="success", sha=SHA, run_id=10):
    return {
        "name": name, "status": status, "conclusion": conclusion,
        "event": "push", "head_sha": sha,
        "created_at": "2026-10-28T00:00:00Z", "id": run_id,
    }


class OneClickReleaseTests(unittest.TestCase):
    def test_only_manual_main_run_can_contact_vps(self):
        self.assertIn('name: "TOR — Обновить сайт"', WORKFLOW)
        self.assertIn("on:\n  workflow_dispatch:\n", WORKFLOW)
        self.assertNotIn("\n  push:", WORKFLOW)
        self.assertNotIn("\n  pull_request:", WORKFLOW)
        self.assertIn("github.ref == 'refs/heads/main'", WORKFLOW)
        self.assertIn("environment: tor-staging", WORKFLOW)
        self.assertNotIn("confirm_sha:", WORKFLOW)
        self.assertNotIn("install:\n", WORKFLOW)
        self.assertIn("actions: read", WORKFLOW)

    def test_green_checks_precede_ssh_and_execution(self):
        self.assertLess(
            WORKFLOW.index("check_one_click_release_ci.py"),
            WORKFLOW.index("Установить SSH"),
        )
        self.assertLess(
            WORKFLOW.index("Запретить установку устаревшего коммита"),
            WORKFLOW.index("sudo -n /usr/local/sbin/tor-one-click-release"),
        )
        self.assertIn("StrictHostKeyChecking=yes", WORKFLOW)
        self.assertIn("UserKnownHostsFile=", WORKFLOW)
        self.assertIn("ssh-keygen -F", WORKFLOW)
        self.assertIn("TARGET_SHA: ${{ github.sha }}", WORKFLOW)
        self.assertIn('--sha "$TARGET_SHA"', WORKFLOW)

    def test_only_fixed_root_owned_executor_migrates(self):
        self.assertIn("sudo -n /usr/local/sbin/tor-one-click-release", WORKFLOW)
        self.assertNotIn("sudo -n /bin/bash", WORKFLOW)
        self.assertIn("chown -R root:root /opt/tor/releases", BOOTSTRAP)
        self.assertIn("install -d -o tor-deploy -g tor-deploy -m 0700 /opt/tor/incoming", BOOTSTRAP)
        self.assertIn("visudo -cf", BOOTSTRAP)
        self.assertIn("tor-deploy ALL=(root) NOPASSWD: /usr/local/sbin/tor-one-click-release *", BOOTSTRAP)
        self.assertIn("$(stat -c '%U:%G' /opt/tor/releases) == root:root", HELPER)
        self.assertIn("$(stat -c '%U:%G' \"$target/tor-migrate\") == root:root", HELPER)
        self.assertIn("flock -n 9", HELPER)

    def test_pinned_sha_and_hashes_before_go_live(self):
        self.assertLess(HELPER.index("main_sha=$("), HELPER.index('bash "$runner" "$sha"'))
        self.assertLess(HELPER.index("sha256sum -c --status tor-web.sha256"), HELPER.index('bash "$runner" "$sha"'))
        self.assertLess(HELPER.index("sha256sum -c --status tor-migrate.sha256"), HELPER.index('bash "$runner" "$sha"'))
        self.assertIn("https://api.github.com/repos/calex794/tor/git/ref/heads/main", HELPER)
        self.assertIn("/opt/tor/incoming/$sha", HELPER)
        self.assertIn("https://raw.githubusercontent.com/calex794/tor/$sha/ops/go-live-postgres-vps.sh", HELPER)
        self.assertIn("require_current_main_sha", GO_LIVE)

    def test_existing_deploy_key_signs_and_server_verifies_binary_manifest(self):
        self.assertIn("ssh-keygen -Y sign", WORKFLOW)
        self.assertIn("ssh-keygen -Y verify", HELPER)
        self.assertIn("/etc/tor/tor-release-allowed-signers", BOOTSTRAP)
        self.assertLess(
            HELPER.index("ssh-keygen -Y verify"),
            HELPER.index('bash "$runner" "$sha"'),
        )
        with tempfile.TemporaryDirectory(prefix="tor-signature-test-") as d:
            directory = Path(d)
            key = directory / "id_ed25519"
            subprocess.run(
                ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)],
                capture_output=True, check=True,
            )
            key_line = (directory / "id_ed25519.pub").read_text().split()
            signers = directory / "allowed_signers"
            signers.write_text(f"tor-release {key_line[0]} {key_line[1]}\n")
            manifest = directory / "manifest"
            manifest.write_text("TOR_RELEASE_SHA " + SHA + "\n")
            subprocess.run(
                ["ssh-keygen", "-Y", "sign", "-f", str(key), "-n", "tor-release", str(manifest)],
                capture_output=True, check=True,
            )
            verify = [
                "ssh-keygen", "-Y", "verify", "-f", str(signers),
                "-I", "tor-release", "-n", "tor-release",
                "-s", str(directory / "manifest.sig"),
            ]
            valid = subprocess.run(
                verify, input=manifest.read_bytes(), capture_output=True,
            )
            self.assertEqual(valid.returncode, 0, valid.stderr)
            altered = subprocess.run(
                verify, input=b"modified manifest\n", capture_output=True,
            )
            self.assertNotEqual(altered.returncode, 0)

    def test_backup_and_rollback_are_real_and_not_ddl_rollback(self):
        self.assertIn("pg_dump -Fc --no-owner --no-privileges", MIGRATE)
        self.assertIn("pg_restore -l", MIGRATE)
        self.assertIn("trap undo ERR", GO_LIVE)
        self.assertIn('mv -Tf "$back" /opt/tor/current', GO_LIVE)
        self.assertIn("systemctl restart tor-web.service", GO_LIVE)
        self.assertIn("migrations are preserved", GO_LIVE)

    def test_new_release_directory_is_traversable_and_config_retained(self):
        # mktemp -d makes 0700 directories; systemd runs as tor-deploy.
        self.assertIn('chmod 0755 "$temporary"', HELPER)
        self.assertLess(
            HELPER.index('chmod 0755 "$temporary"'),
            HELPER.index('mv -T "$temporary" "$target"'),
        )
        self.assertIn('if [[ -f "$dropin" ]]; then', GO_LIVE)
        self.assertIn('grep -Fxq "$line" "$dropin"', GO_LIVE)
        self.assertIn('Environment=TOR_AUTH_ENABLED=1', GO_LIVE)

    def test_operator_shell_syntax(self):
        for script in ("ops/one-click-release-vps.sh", "ops/bootstrap-one-click-release-vps.sh"):
            with self.subTest(script=script):
                result = subprocess.run(
                    ["bash", "-n", str(ROOT / script)],
                    capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_ci_gate_requires_both_exact_main_workflows(self):
        rows = [run("Repository rules"), run("TOR integration CI")]
        self.assertEqual(gate.evaluate(rows, SHA)[0], "pass")
        self.assertEqual(gate.evaluate(rows[:1], SHA)[0], "wait")
        rows[1] = run("TOR integration CI", status="in_progress", conclusion=None)
        self.assertEqual(gate.evaluate(rows, SHA)[0], "wait")
        rows[1] = run("TOR integration CI", conclusion="failure")
        self.assertEqual(gate.evaluate(rows, SHA)[0], "fail")
        self.assertEqual(gate.evaluate([], SHA)[0], "wait")

    def test_ci_gate_rejects_wrong_sha_event_and_unregistered_runs(self):
        for name in ("Repository rules", "TOR integration CI"):
            self.assertEqual(gate.evaluate([run(name, sha="b"*40)], SHA)[0], "wait")
        rows = [run("Repository rules"), run("TOR integration CI")]
        rows[0]["event"] = "pull_request"
        self.assertEqual(gate.evaluate(rows, SHA)[0], "wait")
        rows.append(run("unknown push workflow"))
        self.assertEqual(gate.evaluate(rows, SHA)[0], "fail")

    def test_successful_rerun_wins_not_old_green(self):
        first = run("Repository rules", conclusion="failure", run_id=1)
        newest = run("Repository rules", conclusion="success", run_id=2)
        other = run("TOR integration CI")
        self.assertEqual(gate.evaluate([other, newest, first], SHA)[0], "pass")
        newer_failure = run("Repository rules", conclusion="failure", run_id=3)
        self.assertEqual(gate.evaluate([other, first, newest, newer_failure], SHA)[0], "fail")

    def test_i06_live_main_ruleset_passes_despite_classic_disabled(self):
        branch, rules, effective = active_main_policy()
        self.assertIs(branch["protection"]["enabled"], False)
        self.assertTrue(branch["protected"])
        self.assertEqual(gate.evaluate_branch_policy(branch, rules, effective)[0],
                         "pass")

    def test_i06_classic_protection_requires_both_app_scoped_checks(self):
        branch = protected_main()
        self.assertEqual(gate.evaluate_branch_policy(branch, [], [])[0], "pass")
        for change in ("level", "missing_check", "wrong_app", "disabled"):
            with self.subTest(change=change):
                variant = copy.deepcopy(branch)
                if change == "level":
                    variant["protection"]["required_status_checks"]["enforcement_level"] = "non_admins"
                elif change == "missing_check":
                    variant["protection"]["required_status_checks"]["checks"].pop()
                elif change == "wrong_app":
                    variant["protection"]["required_status_checks"]["checks"][0]["app_id"] = -1
                else:
                    variant["protection"]["enabled"] = False
                self.assertEqual(gate.evaluate_branch_policy(variant, [], [])[0], "fail")

    def test_i06_changed_or_ineffective_ruleset_blocks(self):
        branch, rules, effective = active_main_policy()
        for action in ("disabled", "evaluate", "wrong_branch", "excluded", "changed",
                       "other_source", "bad_target", "missing_pr", "no_delete",
                       "no_force_guard", "bypassed", "can_bypass", "wrong_app",
                       "missing_check", "not_enforced_on_create", "no_effective",
                       "other_ruleset_id", "missing_metadata", "missing_enforcement"):
            with self.subTest(action=action):
                b, rs, eff = copy.deepcopy((branch, rules, effective))
                item = rs[0]
                if action in ("disabled", "evaluate"):
                    item["enforcement"] = action
                elif action == "wrong_branch":
                    item["conditions"]["ref_name"]["include"] = ["refs/heads/other"]
                elif action == "excluded":
                    item["conditions"]["ref_name"]["exclude"] = ["refs/heads/main"]
                elif action == "changed":
                    item["updated_at"] = "2026-11-01T00:00:00Z"
                elif action == "other_source":
                    item["source"] = "attacker/tor"
                elif action == "bad_target":
                    item["target"] = "tag"
                elif action in ("missing_pr", "no_delete", "no_force_guard"):
                    target = {"missing_pr": "pull_request", "no_delete": "deletion",
                              "no_force_guard": "non_fast_forward"}[action]
                    item["rules"] = [row for row in item["rules"] if row["type"] != target]
                elif action == "bypassed":
                    item["bypass_actors"] = [{"actor_type": "RepositoryRole",
                                               "actor_id": 5, "bypass_mode": "always"}]
                elif action == "can_bypass":
                    item["current_user_can_bypass"] = "always"
                elif action == "wrong_app":
                    item["rules"][-1]["parameters"]["required_status_checks"][0]["integration_id"] = -1
                elif action == "missing_check":
                    item["rules"][-1]["parameters"]["required_status_checks"].pop()
                elif action == "not_enforced_on_create":
                    item["rules"][-1]["parameters"]["do_not_enforce_on_create"] = True
                elif action == "no_effective":
                    eff = []
                elif action == "other_ruleset_id":
                    eff[0]["ruleset_id"] = 4567
                elif action == "missing_metadata":
                    item["current_user_can_bypass"] = None
                elif action == "missing_enforcement":
                    item.pop("enforcement")
                self.assertEqual(gate.evaluate_branch_policy(b, rs, eff)[0], "fail")

    def test_i06_explicit_empty_bypass_is_accepted_for_organization(self):
        b, rs, eff = active_main_policy()
        item = rs[0]
        item["id"] = 5000
        item["source_type"] = "Organization"
        item["source"] = "calex794"
        item["conditions"]["repository_name"] = {
            "include": ["tor"], "exclude": [],
        }
        item["bypass_actors"] = []
        for row in eff:
            row["ruleset_id"] = 5000
        self.assertEqual(gate.evaluate_branch_policy(b, rs, eff)[0], "pass")
        item["conditions"]["repository_name"]["exclude"] = ["tor"]
        self.assertEqual(gate.evaluate_branch_policy(b, rs, eff)[0], "fail")

    def test_i06_fails_closed_without_branch_or_policy_api_data(self):
        branch, rules, effective = active_main_policy()
        for value in (None, {}, {"name": "main", "protected": False},
                      {"name": "wrong", "protected": True}):
            with self.subTest(branch=value):
                self.assertEqual(gate.evaluate_branch_policy(value, rules, effective)[0],
                                 "fail")
        for bad in (None, {}, "invalid"):
            self.assertEqual(gate.evaluate_branch_policy(branch, bad, effective)[0], "fail")
            self.assertEqual(gate.evaluate_branch_policy(branch, rules, bad)[0], "fail")
        self.assertEqual(gate.evaluate_branch_policy(branch, [], [])[0], "fail")

    def test_i06_list_and_detail_inconsistent_or_api_error_refuses(self):
        _, rs, _ = active_main_policy()
        detail = rs[0]
        summary = {key: detail[key] for key in
                   ("id", "target", "source_type", "source", "enforcement", "updated_at")}
        with mock.patch.object(gate, "_fetch_json", side_effect=[[summary], detail]):
            self.assertEqual(gate.fetch_rulesets("calex794/tor", "fake"), [detail])
        wrong = dict(detail, updated_at="updated")
        with mock.patch.object(gate, "_fetch_json", side_effect=[[summary], wrong]):
            with self.assertRaises(ValueError):
                gate.fetch_rulesets("calex794/tor", "fake")
        with mock.patch.object(gate, "_fetch_json",
                               side_effect=HTTPError("https://api.github.com", 403,
                                                     "Forbidden", {}, None)):
            with self.assertRaises(HTTPError):
                gate.fetch_rulesets("calex794/tor", "fake")

    def test_one_click_refuses_unprotected_main_before_any_ci_fetch(self):
        argv = ["gate", "--repo", "calex794/tor", "--sha", SHA, "--timeout-seconds", "0"]
        with mock.patch.dict(gate.os.environ, {"GITHUB_TOKEN": "fake"}), \
             mock.patch.object(gate.sys, "argv", argv), \
             mock.patch.object(gate, "require_current_main"), \
             mock.patch.object(gate, "verify_policy",
                               return_value=("fail", "not protected")), \
             mock.patch.object(gate, "fetch_runs") as runs:
            self.assertEqual(gate.main(), 1)
            runs.assert_not_called()

    def test_one_click_refuses_stale_main_and_unavailable_ruleset(self):
        argv = ["gate", "--repo", "calex794/tor", "--sha", SHA, "--timeout-seconds", "0"]
        with mock.patch.dict(gate.os.environ, {"GITHUB_TOKEN": "fake"}), \
             mock.patch.object(gate.sys, "argv", argv), \
             mock.patch.object(gate, "require_current_main",
                               side_effect=gate.EvidenceError("main moved")), \
             mock.patch.object(gate, "verify_policy") as policy:
            self.assertEqual(gate.main(), 1)
            policy.assert_not_called()
        with mock.patch.dict(gate.os.environ, {"GITHUB_TOKEN": "fake"}), \
             mock.patch.object(gate.sys, "argv", argv), \
             mock.patch.object(gate, "require_current_main"), \
             mock.patch.object(gate, "verify_policy",
                               side_effect=HTTPError("https://api.github.com", 403,
                                                     "Forbidden", {}, None)):
            self.assertEqual(gate.main(), 1)

    def test_one_click_does_not_trust_a_green_wrapper_without_suite_evidence(self):
        argv = ["gate", "--repo", "calex794/tor", "--sha", SHA, "--timeout-seconds", "0"]
        rows = [run("Repository rules"), run("TOR integration CI")]
        with mock.patch.dict(gate.os.environ, {"GITHUB_TOKEN": "fake"}), \
             mock.patch.object(gate.sys, "argv", argv), \
             mock.patch.object(gate, "require_current_main"), \
             mock.patch.object(gate, "verify_policy", return_value=("pass", "protected")), \
             mock.patch.object(gate, "fetch_runs", return_value=rows), \
             mock.patch.object(gate, "collect", return_value={
                 "ci_complete": False, "blockers": ["missing postgres-runtime"]}):
            self.assertEqual(gate.main(), 1)

    def test_one_click_rechecks_rulesets_after_all_jobs_pass(self):
        argv = ["gate", "--repo", "calex794/tor", "--sha", SHA, "--timeout-seconds", "0"]
        rows = [run("Repository rules"), run("TOR integration CI")]
        with mock.patch.dict(gate.os.environ, {"GITHUB_TOKEN": "fake"}), \
             mock.patch.object(gate.sys, "argv", argv), \
             mock.patch.object(gate, "require_current_main"), \
             mock.patch.object(gate, "verify_policy",
                               side_effect=[("pass", "protected"), ("fail", "changed")]) as pol, \
             mock.patch.object(gate, "fetch_runs", return_value=rows), \
             mock.patch.object(gate, "collect", return_value={
                 "ci_complete": True, "blockers": []}):
            self.assertEqual(gate.main(), 1)
            self.assertEqual(pol.call_count, 2)

    def test_policy_only_never_fetches_ci_or_connects_ssh(self):
        argv = ["gate", "--repo", "calex794/tor", "--sha", SHA,
                "--timeout-seconds", "0", "--policy-only"]
        with mock.patch.dict(gate.os.environ, {"GITHUB_TOKEN": "fake"}), \
             mock.patch.object(gate.sys, "argv", argv), \
             mock.patch.object(gate, "require_current_main") as ref, \
             mock.patch.object(gate, "verify_policy", return_value=("pass", "protected")), \
             mock.patch.object(gate, "fetch_runs") as runs, \
             mock.patch.object(gate, "collect") as audit:
            self.assertEqual(gate.main(), 0)
            self.assertEqual(ref.call_count, 2)
            runs.assert_not_called()
            audit.assert_not_called()



if __name__ == "__main__":
    unittest.main()
