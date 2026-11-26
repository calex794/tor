"""Fixture-only integration of the signed recovery release into real PG HTTP."""
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import release_bundle_v2 as bundle
import stage_recovery_ci_release as fixture
import tor_recovery_auth as recovery_auth

SHA = "a" * 40
SMOKE = (ROOT / "tools/tor_pg_backup_restore_smoke.sh").read_text()
WORKFLOW = (ROOT / ".github/workflows/tor-integration.yml").read_text()


class RecoveryHTTPTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        self.work = self.root / "tor-pg-restore.fixture"
        self.work.mkdir(mode=0o700)
        self.env = {**fixture.EXPECTED, "RUNNER_TEMP": str(self.root)}

    def test_only_disposable_environment_and_private_workspace(self):
        fixture.check_environment(self.env, self.work)
        for name in fixture.EXPECTED:
            with self.subTest(name=name), self.assertRaises(fixture.Refused):
                fixture.check_environment({**self.env, name: "wrong"}, self.work)
        self.work.chmod(0o755)
        with self.assertRaises(fixture.Refused):
            fixture.check_environment(self.env, self.work)

    def test_signed_roundtrip_uses_real_elf_and_leaves_no_private_key(self):
        binaries = self.root / "bin"
        binaries.mkdir()
        for name in bundle.BINARIES:
            shutil.copy2("/usr/bin/true", binaries / name)
        before = (binaries / "tor-web").read_bytes()
        runtime = fixture.prepare(self.work, binaries, SHA)
        self.assertEqual({p.name for p in self.work.iterdir()}, {"recovered-runtime"})
        self.assertEqual({p.name for p in runtime.iterdir()}, bundle.FILES)
        for name in bundle.FILES:
            self.assertEqual(stat.S_IMODE((runtime / name).stat().st_mode),
                             0o755 if name in bundle.BINARIES else 0o644)
        subprocess.run([str(runtime / "tor-web")], check=True, timeout=5)
        self.assertEqual((binaries / "tor-web").read_bytes(), before)
        with self.assertRaises(fixture.Refused):
            fixture.prepare(self.work, binaries, SHA)

    def test_real_restore_uses_verified_binary_and_clean_environment(self):
        subprocess.run(["bash", "-n", str(ROOT / "tools/tor_pg_backup_restore_smoke.sh")], check=True)
        self.assertLess(SMOKE.index("postgres:18 pg_restore"), SMOKE.index("stage_recovery_ci_release.py"))
        self.assertIn('web_binary="$work_dir/recovered-runtime/tor-web"', SMOKE)
        self.assertIn("env -i PATH=/usr/bin:/bin", SMOKE)
        self.assertIn('"/proc/$web_pid/exe" -ef "$web_binary"', SMOKE)
        self.assertIn('[[ "$private_code" == 404 ]]', SMOKE)
        self.assertIn('"$post_start_history" == "$source_history"', SMOKE)
        self.assertIn("DEPLOY07B_SIGNED_V2_RESTORED_PG_HTTP_OK", SMOKE)
        self.assertIn("I11_PG18_RESTORE_SOURCE_UNCHANGED_OK", SMOKE)
        self.assertLess(SMOKE.index("s061_session_continuity.py seed"), SMOKE.index("postgres:18 pg_dump"))
        self.assertLess(SMOKE.index("postgres:18 pg_restore"), SMOKE.index("s061_session_continuity.py check"))
        self.assertIn("AUTH08_PG18_RESTORED_SESSIONS_OK", SMOKE)
        step = WORKFLOW.split("I-11 PostgreSQL 18 backup and clean restore drill", 1)[1].split("      - name:", 1)[0]
        self.assertIn("matrix.shard == 'foundation'", step)
        self.assertIn("cargo build -p tor-observability --bin tor-trace-store", step)
        self.assertIn("bash tools/tor_pg_backup_restore_smoke.sh", step)

    def test_auth_recovery_refuses_unsafe_identity(self):
        for database in ("postgres", "template1", "tor;DROP", "TOR", ""):
            with self.subTest(database=database), self.assertRaises(recovery_auth.Refused):
                recovery_auth.operator_guard(database, "123456789", 15432)
        for identity in ("0", "-1", "abc", "18446744073709551616"):
            with self.subTest(identity=identity), self.assertRaises(recovery_auth.Refused):
                recovery_auth.operator_guard("tor", identity, 15432)
        with self.assertRaises(recovery_auth.Refused):
            recovery_auth.operator_guard("tor", "123456789", 5432)

    def test_auth_recovery_requires_atomic_guard_and_offline_plan(self):
        guard = recovery_auth.operator_guard("tor", "123456789", 15432)
        for check in ("NOT pg_is_in_recovery()", "inet_server_addr() IS NULL",
                      "current_database() = 'tor'", "pg_control_system()",
                      "pg_stat_activity", "IS NOT TRUE"):
            self.assertIn(check, guard)
        plan = recovery_auth.plan_sql(guard)
        self.assertIn("BEGIN READ ONLY", plan)
        self.assertNotIn("UPDATE public.sessions", plan)
        statement = recovery_auth.invalidation_sql(guard)
        self.assertEqual(statement.count(guard), 2)
        self.assertLess(statement.index(guard), statement.index("LOCK TABLE"))
        self.assertLess(statement.rindex(guard), statement.index("UPDATE public.sessions"))
        self.assertLess(statement.index("AUTH08C live sessions remain"), statement.index("COMMIT;"))
        self.assertIn("ACCESS EXCLUSIVE", statement)
        self.assertIn("WHERE " + recovery_auth.ACTIVE, statement)
        continuity = (ROOT / "tools/s061_session_continuity.py").read_text()
        self.assertIn("revoke_restored = invalidation_sql()", continuity)


    def test_auth_recovery_receipt_pins_and_lineage(self):
        from hashlib import sha256
        from types import SimpleNamespace
        from unittest.mock import patch
        import json

        args = SimpleNamespace(source_machine_id="a" * 32,
                               target_machine_id="b" * 32, pg_port=15432)
        stage = {
            "schema_version": 1, "scope": "physical_files_staged_only",
            "source_machine_id": args.source_machine_id,
            "target_machine_id": args.target_machine_id,
            "pg_port": args.pg_port, "backup_label": "20261126-000001F",
            "target_utc": "2026-11-26T00:01:00Z", "timeline": 1,
        }
        def encode(record):
            return json.dumps(record, sort_keys=True).encode("ascii")

        stage_raw = encode(stage)
        args.stage_sha256 = sha256(stage_raw).hexdigest()
        replay = {
            "schema_version": 1,
            "scope": "paused_replay_observed_then_stopped_only",
            "stage_receipt_sha256": args.stage_sha256, "pg_port": args.pg_port,
            "backup_label": stage["backup_label"], "target_utc": stage["target_utc"],
            "timeline": stage["timeline"], "stable_paused_replay_observed": True,
            "postgres_shutdown_confirmed": True, "promotion_authorized": False,
            "recovery_accepted": False,
        }
        def verify(stage_bytes, replay_bytes):
            with patch.object(recovery_auth, "private_read",
                              side_effect=[stage_bytes, replay_bytes]):
                recovery_auth.verify_receipts(self.root, args)

        good_replay = encode(replay)
        args.replay_sha256 = sha256(good_replay).hexdigest()
        verify(stage_raw, good_replay)
        for a, b in ((stage_raw + b" ", good_replay), (stage_raw, good_replay + b" ")):
            with self.subTest(tamper=True), self.assertRaises(recovery_auth.Refused):
                verify(a, b)
        for key, bad in (
            ("stage_receipt_sha256", "c" * 64), ("timeline", 2),
            ("pg_port", 15433), ("stable_paused_replay_observed", False),
            ("postgres_shutdown_confirmed", False), ("promotion_authorized", True),
            ("recovery_accepted", True), ("schema_version", True),
        ):
            wrong = encode({**replay, key: bad})
            args.replay_sha256 = sha256(wrong).hexdigest()
            with self.subTest(key=key), self.assertRaises(recovery_auth.Refused):
                verify(stage_raw, wrong)
        duplicate = good_replay[:-1] + b', "schema_version": 1}'
        args.replay_sha256 = sha256(duplicate).hexdigest()
        with self.assertRaises(recovery_auth.Refused):
            verify(stage_raw, duplicate)


if __name__ == "__main__":
    unittest.main()
