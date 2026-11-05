"""S061: release drill refusal, atomicity and workflow integration guards."""
from pathlib import Path
import importlib.util
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "tools" / "s061_cutover_pg_smoke.py"
SPEC = importlib.util.spec_from_file_location("s061_cutover_pg_smoke", SOURCE)
assert SPEC and SPEC.loader
DRILL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DRILL)


class CandidateCutoverGuardTests(unittest.TestCase):
    def test_production_database_or_non_github_runtime_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            good = {**DRILL.EXPECTED, "RUNNER_TEMP": temp}
            self.assertEqual(DRILL.require_disposable(good), Path(temp))
            for key, value in (
                ("GITHUB_ACTIONS", ""),
                ("PGHOST", "db.example.test"),
                ("PGUSER", "tor_app"),
                ("PGDATABASE", "tor"),
                ("TEST_DATABASE_URL", "postgresql://tor_app:secret@127.0.0.1:5432/tor"),
                ("MIGRATOR_URL", "postgresql://tor_migrator:secret@127.0.0.1:5432/tor"),
                ("RUNNER_TEMP", "../tmp"),
            ):
                with self.subTest(key=key):
                    changed = dict(good, **{key: value})
                    with self.assertRaises(RuntimeError):
                        DRILL.require_disposable(changed)

    def test_distinct_binary_requires_runner_local_different_executable(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            new = root / "new"
            old = root / "old"
            new.write_bytes(b"new Rust binary")
            old.write_bytes(b"previous Rust binary")
            for binary in (new, old):
                binary.chmod(0o700)
            self.assertEqual(DRILL.select_previous_binary({}, root, new), (new, False))
            self.assertEqual(DRILL.select_previous_binary(
                {"TOR_S061_PREVIOUS_BINARY": str(old)}, root, new), (old, True))
            old.write_bytes(new.read_bytes())
            with self.assertRaises(RuntimeError):
                DRILL.select_previous_binary({"TOR_S061_PREVIOUS_BINARY": str(old)}, root, new)
            outside = root.parent / "out-of-runner-old"
            with self.assertRaises(RuntimeError):
                DRILL.select_previous_binary(
                    {"TOR_S061_PREVIOUS_BINARY": str(outside)}, root, new)
            old.unlink()
            old.symlink_to(new)
            with self.assertRaises(RuntimeError):
                DRILL.select_previous_binary({"TOR_S061_PREVIOUS_BINARY": str(old)}, root, new)

    def test_atomic_release_pointer_only_accepts_private_release_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            releases = root / "releases"
            previous = releases / ("a" * 40)
            candidate = releases / ("b" * 40)
            previous.mkdir(parents=True)
            candidate.mkdir()
            pointer = root / "current"
            pointer.symlink_to(previous)
            DRILL.atomic_select(pointer, candidate)
            self.assertEqual(pointer.resolve(), candidate)
            DRILL.atomic_select(pointer, previous)
            self.assertEqual(pointer.resolve(), previous)
            outside = root / "outside"
            outside.mkdir()
            with self.assertRaises(RuntimeError):
                DRILL.atomic_select(pointer, outside)
            self.assertEqual(pointer.resolve(), previous)
            candidate.rename(root / "moved")
            with self.assertRaises(RuntimeError):
                DRILL.atomic_select(pointer, candidate)
            self.assertEqual(pointer.resolve(), previous)

    def test_both_pg_jobs_run_same_actual_script_without_extra_cargo_build(self):
        required = ROOT / ".github" / "workflows" / "tor-integration.yml"
        manual = ROOT / ".github" / "workflows" / "tor-pg-runtime.yml"
        for path in (required, manual):
            with self.subTest(workflow=path.name):
                text = path.read_text()
                self.assertIn("python3 tools/s061_cutover_pg_smoke.py", text)
                self.assertIn("test -x ./target/debug/tor-web", text)
                self.assertIn("s061-real-cutover/report.json", text)
                self.assertIn("name: s061-real-cutover-pg", text)
                self.assertLess(text.index("bash tools/tor_pg_backup_restore_smoke.sh"),
                                text.index("python3 tools/s061_cutover_pg_smoke.py"))
        mapper = (ROOT / "tools" / "ci_test_map.py").read_text()
        self.assertIn('"tools/s061_cutover_pg_smoke.py": {"postgres"}', mapper)

    def test_drill_isolated_and_declares_non_zero_downtime_limit(self):
        text = SOURCE.read_text()
        self.assertIn("GITHUB_ACTIONS", text)
        self.assertIn("atomic_select(current, candidate)", text)
        self.assertIn("atomic_select(current, previous)", text)
        self.assertIn("stop(promoted, force=True)", text)
        self.assertIn("migration_state() == before", text)
        self.assertIn('"same_binary_both_slots": not distinct', text)
        self.assertIn('"distinct_binary_validation": distinct', text)
        self.assertIn('"no_production_or_tls_staging_claim": True', text)
        self.assertNotIn("TOR_DEPLOY_SSH_KEY", text)
        self.assertNotIn("pandaolate.ru", text)


if __name__ == "__main__":
    unittest.main()
