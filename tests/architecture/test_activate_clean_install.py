"""Durable first-install phase ordering, refusal and non-destructive failure."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import activate_clean_install as activation
import prepare_clean_install as clean
import release_bundle_v2 as bundle

RECORD = clean.plan("a" * 40, "b" * 32, "c" * 64)


class Fixture:
    def __init__(self, journal, fail=None):
        self.journal, self.fail = journal, fail
        self.calls = []

    def step(self, phase):
        self.calls.append(phase)
        if phase not in ("preflight", "stop"):
            data = json.loads(self.journal.path.read_bytes())
            assert data["phase"] == phase and data["state"] == "active"
        if self.fail == phase:
            raise RuntimeError("injected")

    def preflight(self):
        self.step("preflight")

    def stage(self):
        self.step("stage")

    def database(self):
        self.step("database")

    def configure_edge(self):
        self.step("edge")

    def start(self):
        self.step("application")

    def verify(self):
        self.step("verify")

    def publish(self):
        self.step("publish")

    def stop_unaccepted(self):
        self.step("stop")


class ActivationTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)

    def fixture(self, directory=None, fail=None):
        journal = activation.ActivationJournal(directory or self.root)
        return journal, Fixture(journal, fail)

    def test_durable_intent_before_every_step(self):
        journal, ops = self.fixture()
        activation.run(RECORD, journal, ops)
        self.assertEqual(ops.calls, ["preflight", *activation.PHASES])
        state = json.loads(journal.path.read_bytes())
        self.assertEqual(state["state"], "local_ready")
        self.assertFalse(state["public_accepted"])
        self.assertEqual(journal.path.stat().st_mode & 0o777, 0o600)

    def test_preflight_refusal_changes_nothing(self):
        journal, ops = self.fixture(fail="preflight")
        with self.assertRaises(RuntimeError):
            activation.run(RECORD, journal, ops)
        self.assertFalse(journal.path.exists())
        self.assertEqual(ops.calls, ["preflight"])

    def test_failed_phases_never_reinitialize(self):
        for phase in activation.PHASES:
            with self.subTest(phase=phase):
                directory = self.root / phase
                directory.mkdir()
                journal, ops = self.fixture(directory, phase)
                with self.assertRaisesRegex(activation.Refused, "FIRST_START_INCOMPLETE"):
                    activation.run(RECORD, journal, ops)
                state = json.loads(journal.path.read_bytes())
                self.assertEqual((state["phase"], state["state"]), (phase, "incomplete"))
                self.assertEqual("stop" in ops.calls, phase in ("application", "verify", "publish"))
                retry = Fixture(journal)
                with self.assertRaises(activation.Refused):
                    activation.run(RECORD, journal, retry)
                self.assertEqual(retry.calls, ["preflight"])

    def test_unexpected_interruption_leaves_active_intent(self):
        class Crash(BaseException):
            pass
        journal, ops = self.fixture()
        def crash():
            raise Crash()
        ops.database = crash
        with self.assertRaises(Crash):
            activation.run(RECORD, journal, ops)
        state = json.loads(journal.path.read_bytes())
        self.assertEqual((state["phase"], state["state"]), ("database", "active"))
        with self.assertRaises(activation.Refused):
            activation.run(RECORD, journal, Fixture(journal))

    def test_new_file_refuses_existing_path_and_symlink(self):
        path = self.root / "config"
        activation.new_file(path, b"original")
        with self.assertRaises(FileExistsError):
            activation.new_file(path, b"changed")
        alias = self.root / "alias"
        alias.symlink_to(path)
        with self.assertRaises(FileExistsError):
            activation.new_file(alias, b"changed")
        self.assertEqual(path.read_bytes(), b"original")

    def test_listener_guard(self):
        activation.check_bindings("LISTEN 0 64 127.0.0.1:5432 0.0.0.0:*\n", {"5432"})
        for addr in ("0.0.0.0:443", "[::]:443", "*:443", "10.0.0.1:443"):
            with self.assertRaises(activation.Refused):
                activation.check_bindings("LISTEN 0 64 " + addr + " 0.0.0.0:*\n", {"443"})

    def test_signed_database_scripts_keep_legacy_and_explicit_empty_guards(self):
        for name in ("init-tor-database.sh", "apply-tor-migrations-vps.sh"):
            source = ROOT / bundle.SCRIPTS[name]
            subprocess.run(["bash", "-n", str(source)], check=True)
            text = source.read_text()
            self.assertIn("--install-empty", text)
            self.assertIn("/srv/tor-install/activation.json", text)
            self.assertIn("/var/lib/tor-release/pg-activated", text)
            self.assertIn("systemctl is-active tor-web.service", text)
        migration = (ROOT / bundle.SCRIPTS["apply-tor-migrations-vps.sh"]).read_text()
        self.assertIn("clean install requires empty schema", migration)
        self.assertIn("pg_dump -Fc", migration)
        self.assertIn("REVOKE INSERT, UPDATE, DELETE ON TABLE public._sqlx_migrations", migration)

    def test_database_failure_exposes_only_fixed_step_and_exit(self):
        journal, ops = self.fixture()
        def fail():
            raise activation.DatabaseStepRefused("init-tor-database.sh", 17)
        ops.database = fail
        with self.assertRaisesRegex(activation.Refused,
                                    "FIRST_START_INCOMPLETE: database;.*database_step=init:app_tcp_acl exit=17"):
            activation.run(RECORD, journal, ops)
        self.assertEqual(json.loads(journal.path.read_bytes())["state"], "incomplete")
        self.assertNotIn("secret-password", str(activation.DatabaseStepRefused("secret-password", 1)))
        self.assertIn("database_step=migrate exit=1",
                      str(activation.DatabaseStepRefused("apply-tor-migrations-vps.sh", 1)))

    def test_signed_script_discards_secret_output_and_reports_exit(self):
        ops = activation.Operations(self.root, RECORD, self.root / "signers", 9)
        with patch.object(activation.bundle, "verify"), \
             patch.object(activation.subprocess, "Popen") as popen:
            popen.return_value.wait.return_value = 18
            with self.assertRaisesRegex(activation.DatabaseStepRefused, "init:migrator_tcp_acl exit=18"):
                ops.signed_script("init-tor-database.sh", "--install-empty")
            kwargs = popen.call_args.kwargs
            self.assertEqual(kwargs["stdout"], subprocess.DEVNULL)
            self.assertEqual(kwargs["stderr"], subprocess.DEVNULL)
            self.assertEqual(kwargs["pass_fds"], (9,))

    def test_init_acl_queries_do_not_coerce_boolean_to_text(self):
        source = (ROOT / bundle.SCRIPTS["init-tor-database.sh"]).read_text()
        self.assertEqual(source.count("SELECT current_user, current_database(), has_schema_privilege"), 2)
        self.assertEqual(source.count("psql -X -w -Atq -F ':' -v ON_ERROR_STOP=1 -c"), 2)
        self.assertNotIn("current_database() || ':'", source)
        for marker in ("grep -Fxq 'tor_app:tor:f'", "grep -Fxq 'tor_migrator:tor:t'"):
            self.assertIn(marker, source)
        for number, label in activation.INIT_FAILURES.items():
            self.assertIn("init_step=" + str(number), source)
            self.assertIn(label, str(activation.DatabaseStepRefused("init-tor-database.sh", number)))
        self.assertIn("trap 'exit \"$init_step\"' ERR", source)
        self.assertIn("exit \"$init_step\"", source)
        self.assertIn("fail_init()", source)

    def test_initial_vhost_never_binds_public_interfaces(self):
        nginx = (ROOT / bundle.INSTALL["install-nginx.conf"]).read_text()
        listen = [line.strip() for line in nginx.splitlines() if line.strip().startswith("listen ")]
        self.assertEqual(len(listen), 4)
        self.assertTrue(all(line.startswith("listen 127.0.0.1:") for line in listen))


if __name__ == "__main__":
    unittest.main()
