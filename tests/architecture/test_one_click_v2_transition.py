"""Protocol transition stays manual-only, verified and fail-closed."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import install_release_v2 as install

WORKFLOW = (ROOT / ".github/workflows/tor-one-click-release.yml").read_text()
WRAPPER = (ROOT / "ops/one-click-release-v2-wrapper.sh").read_bytes()
V1 = b"#!/bin/bash\n# Installed by ops/bootstrap-one-click-release-vps.sh\n"


class TransitionTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.wrapper = self.root / "helper"
        self.wrapper.write_bytes(V1)
        self.wrapper.chmod(0o755)
        self.backup = self.root / "backup"

    def test_v1_is_backed_up_before_wrapper_swap_and_retry_is_safe(self):
        install.disable_v1(self.wrapper, self.backup, WRAPPER)
        self.assertEqual(self.backup.read_bytes(), V1)
        self.assertEqual(self.backup.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.wrapper.read_bytes(), WRAPPER)
        self.assertEqual(self.wrapper.stat().st_mode & 0o777, 0o755)
        install.disable_v1(self.wrapper, self.backup, WRAPPER)
        self.assertEqual(self.backup.read_bytes(), V1)

    def test_unknown_v1_helper_not_overwritten(self):
        self.wrapper.write_bytes(b"unknown")
        with self.assertRaises(install.InstallError):
            install.disable_v1(self.wrapper, self.backup, WRAPPER)
        self.assertFalse(self.backup.exists())
        self.assertEqual(self.wrapper.read_bytes(), b"unknown")

    def test_wrong_existing_backup_refused(self):
        self.backup.write_bytes(b"wrong")
        self.backup.chmod(0o600)
        with self.assertRaises(install.InstallError):
            install.disable_v1(self.wrapper, self.backup, WRAPPER)
        self.assertEqual(self.wrapper.read_bytes(), V1)

    def test_marker_is_atomic_and_never_overwritten(self):
        marker = self.root / "release-format"
        install.publish_marker(marker)
        self.assertEqual(marker.read_bytes(), b"2\n")
        self.assertEqual(marker.stat().st_mode & 0o777, 0o644)
        with self.assertRaises(install.InstallError):
            install.publish_marker(marker)

    def test_helpers_are_private_immutable_and_idempotent(self):
        files = {name: b"fixture " + name.encode() for name in install.MODULES}
        directory = self.root / "helpers"
        install.install_helpers(directory, files)
        self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
        install.install_helpers(directory, files)
        for name in files:
            self.assertEqual((directory / name).stat().st_mode & 0o777, 0o600)
        files[install.MODULES[0]] = b"changed"
        with self.assertRaises(install.InstallError):
            install.install_helpers(directory, files)

    def test_shell_syntax_and_wrapper_refuses_non_sha(self):
        for path in ("ops/bootstrap-one-click-v2.sh", "ops/one-click-release-v2-wrapper.sh"):
            subprocess.run(["bash", "-n", str(ROOT / path)], check=True)
        for args in ([], ["--recover"], ["a" * 40, "extra"]):
            result = subprocess.run(["bash", str(ROOT / "ops/one-click-release-v2-wrapper.sh"), *args],
                                    capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 2)

    def test_stage_only_never_enters_privileged_release_or_final_acceptance(self):
        self.assertIn("prepare_v2:\n", WORKFLOW)
        self.assertIn("default: false", WORKFLOW)
        self.assertEqual(WORKFLOW.count("if: inputs.prepare_v2 == false"), 2)
        for name in ("Резервная копия PostgreSQL", "Проверить установленный SHA"):
            block = WORKFLOW.split(name, 1)[1].split("      - name:", 1)[0]
            self.assertIn("if: inputs.prepare_v2 == false", block)
        self.assertIn("TOR_V2_PREPARED_ONLY", WORKFLOW)

    def test_pending_bootstrap_cannot_downgrade(self):
        self.assertIn('printf "pending-v2\\n"', WORKFLOW)
        self.assertIn('"$protocol" == pending-v2', WORKFLOW)
        self.assertIn('grep -Fxq "# TOR RELEASE PROTOCOL 2" "$helper"', WORKFLOW)

    def test_exact_checkout_real_edge_gate_precedes_v2_transfer(self):
        self.assertLess(WORKFLOW.index("python3 tests/edge/managed_edge_smoke.py"),
                        WORKFLOW.index("Собрать и подписать весь комплект v2"))
        self.assertIn('grep -Fxq "TOR_ONE_CLICK_RELEASE_V2_OK $TARGET_SHA"', WORKFLOW)
        self.assertIn('grep -Fxq "TOR_V2_ALREADY_ACTIVE $TARGET_SHA"', WORKFLOW)

    def test_v2_namespace_and_transport_are_separate(self):
        self.assertIn('if: env.TOR_RELEASE_PROTOCOL == \'v2\'', WORKFLOW)
        self.assertIn('tor-release-v2 "$RUNNER_TEMP/tor-v2/tor-release-manifest-v2"', WORKFLOW)
        self.assertIn("/opt/tor/incoming/v2/$TARGET_SHA", WORKFLOW)
        self.assertIn("TOR_V2_SETUP_REQUIRED: no downgrade to v1", WORKFLOW)
        self.assertLess(WORKFLOW.index("check_one_click_release_ci.py"),
                        WORKFLOW.index("Определить установленный протокол"))
        self.assertNotIn(b"one-click-release-vps.sh", WRAPPER)
        self.assertIn(b"release-format", WRAPPER)
        self.assertIn(b"python3 -I -B", WRAPPER)


if __name__ == "__main__":
    unittest.main()
