"""One-button TOR release contracts: manual-only, green CI, safe root boundary."""

from pathlib import Path
import importlib.util
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
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


def protected_main(enabled=True, level="everyone", contexts=("rules",)):
    return {
        "name": "main",
        "protected": enabled,
        "protection": {
            "enabled": enabled,
            "required_status_checks": {
                "enforcement_level": level,
                "contexts": list(contexts),
                "checks": [],
            },
        },
    }


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

    def test_ci_gate_passes_same_main_sha(self):
        rows = [run("Repository rules"), run("TOR PostgreSQL runtime"), run("Rust S0 browser preview")]
        self.assertEqual(gate.evaluate(rows, SHA)[0], "pass")

    def test_ci_gate_refuses_failed_pending_and_missing(self):
        rows = [run("Repository rules"), run("TOR PostgreSQL runtime", conclusion="failure")]
        self.assertEqual(gate.evaluate(rows, SHA)[0], "fail")
        rows[1] = run("TOR PostgreSQL runtime", status="in_progress", conclusion=None)
        self.assertEqual(gate.evaluate(rows, SHA)[0], "wait")
        self.assertEqual(gate.evaluate([], SHA)[0], "wait")

    def test_ci_gate_refuses_wrong_sha_and_pr_event(self):
        self.assertEqual(gate.evaluate([run("Repository rules", sha="b" * 40)], SHA)[0], "wait")
        other = run("Repository rules")
        other["event"] = "pull_request"
        self.assertEqual(gate.evaluate([other], SHA)[0], "wait")

    def test_later_successful_rerun_wins(self):
        earlier = run("Repository rules", conclusion="failure", run_id=1)
        later = run("Repository rules", conclusion="success", run_id=2)
        self.assertEqual(gate.evaluate([later, earlier], SHA)[0], "pass")


    def test_i06_live_main_is_confirmed_unprotected_not_unknown(self):
        # Exact negative metadata returned by GitHub for TOR on 29 Oct.
        branch = protected_main(enabled=False, level="off", contexts=())
        status, why = gate.evaluate_branch_policy(branch)
        self.assertEqual(status, "fail")
        self.assertIn("unprotected", why)

    def test_i06_requires_actual_rules_job_and_admin_enforcement(self):
        self.assertEqual(gate.evaluate_branch_policy(protected_main())[0], "pass")
        for changes in ({"level": "off"}, {"level": "non_admins"},
                        {"contexts": ()}, {"enabled": False}):
            with self.subTest(changes=changes):
                self.assertEqual(gate.evaluate_branch_policy(
                    protected_main(**changes))[0], "fail")

    def test_i06_branch_api_may_omit_optional_enabled_property(self):
        branch = protected_main()
        del branch["protection"]["enabled"]
        self.assertEqual(gate.evaluate_branch_policy(branch)[0], "pass")

    def test_i06_rejects_missing_or_malformed_branch_response(self):
        for bad in (None, {}, {"name": "other", "protected": True},
                    {"name": "main", "protected": True, "protection": {}},
                    {"name": "main", "protected": True,
                     "protection": {"enabled": True, "required_status_checks":
                                    {"enforcement_level": "everyone", "contexts": "rules"}}}):
            with self.subTest(branch=bad):
                self.assertEqual(gate.evaluate_branch_policy(bad)[0], "fail")

    def test_i06_check_name_may_be_in_new_github_checks_array(self):
        branch = protected_main(contexts=())
        branch["protection"]["required_status_checks"]["checks"] = [
            {"context": "rules", "app_id": 15368}]
        self.assertEqual(gate.evaluate_branch_policy(branch)[0], "pass")

    def test_one_click_refuses_unprotected_main_before_any_ci_fetch(self):
        argv = ["gate", "--repo", "calex794/tor", "--sha", SHA, "--timeout-seconds", "0"]
        with mock.patch.dict(gate.os.environ, {"GITHUB_TOKEN": "fake"}), \
             mock.patch.object(gate.sys, "argv", argv), \
             mock.patch.object(gate, "fetch_branch", return_value=protected_main(enabled=False)), \
             mock.patch.object(gate, "fetch_runs") as runs:
            self.assertEqual(gate.main(), 1)
            runs.assert_not_called()

    def test_one_click_rechecks_protection_after_green_ci(self):
        argv = ["gate", "--repo", "calex794/tor", "--sha", SHA, "--timeout-seconds", "0"]
        with mock.patch.dict(gate.os.environ, {"GITHUB_TOKEN": "fake"}), \
             mock.patch.object(gate.sys, "argv", argv), \
             mock.patch.object(gate, "fetch_branch",
                               side_effect=[protected_main(), protected_main(enabled=False)]) as branch, \
             mock.patch.object(gate, "fetch_runs", return_value=[run("Repository rules")]):
            self.assertEqual(gate.main(), 1)
            self.assertEqual(branch.call_count, 2)



if __name__ == "__main__":
    unittest.main()
