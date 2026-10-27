"""Exercise the actual go-live rollback hook with fake services, no VPS."""

from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SOURCE = (ROOT / "ops/go-live-postgres-vps.sh").read_text(encoding="utf-8")
OLD_SHA = "a" * 40
NEW_SHA = "b" * 40


def actual_undo_function():
    start = SOURCE.index("undo() {\n")
    end = SOURCE.index("\ntrap undo ERR", start)
    return SOURCE[start:end]


def executable(path, content):
    path.write_text(content, encoding="utf-8")
    path.chmod(0o755)


class OneClickGoLiveRollbackTests(unittest.TestCase):
    def exercise(self, mode):
        with tempfile.TemporaryDirectory(prefix="tor-go-live-rollback-") as tmp:
            root = Path(tmp) / "opt/tor"
            old = root / "releases" / OLD_SHA
            candidate = root / "releases" / NEW_SHA
            old.mkdir(parents=True)
            candidate.mkdir()
            executable(old / "tor-web", "#!/bin/sh\nexit 0\n")
            executable(candidate / "tor-web", "#!/bin/sh\nexit 0\n")
            current = root / "current"
            current.symlink_to(candidate)
            dropin = Path(tmp) / "service.conf"
            dropin.write_text("new\n", encoding="utf-8")
            bin_dir = Path(tmp) / "bin"
            bin_dir.mkdir()
            executable(bin_dir / "systemctl", """#!/usr/bin/env bash
if [[ "$1" == is-active ]]; then
  [[ "$TOR_TEST_MODE" == success ]]
elif [[ "$1" == show ]]; then
  echo 'ActiveState=failed'
fi
exit 0
""")
            executable(bin_dir / "curl", """#!/usr/bin/env bash
[[ "$TOR_TEST_MODE" == success ]] || exit 7
printf 'ok\n'
""")
            undo = actual_undo_function().replace("/opt/tor", str(root))
            undo = undo.replace("for attempt in $(seq 1 15)", "for attempt in $(seq 1 1)")
            undo = undo.replace("sleep 1", ":")
            run = Path(tmp) / "try.sh"
            executable(run, "\n".join([
                "#!/usr/bin/env bash",
                "set -Eeuo pipefail",
                "old_target=" + str(old),
                "dropin=" + str(dropin),
                'backup_dropin=""',
                "switched=1",
                undo,
                "undo",
            ]) + "\n")
            syntax = subprocess.run(["bash", "-n", str(run)], capture_output=True, text=True)
            self.assertEqual(syntax.returncode, 0, syntax.stderr)
            env = os.environ.copy()
            env.update({
                "PATH": str(bin_dir) + ":" + env.get("PATH", ""),
                "TOR_TEST_MODE": mode,
            })
            result = subprocess.run(
                ["bash", str(run)], env=env, text=True,
                capture_output=True, timeout=8,
            )
            return result, current.resolve(), old

    def test_previous_binary_and_https_confirm_rollback(self):
        result, active, old = self.exercise("success")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(active, old)
        self.assertIn("ROLLBACK_VERIFIED:", result.stderr)
        self.assertNotIn("ROLLBACK_UNVERIFIED:", result.stderr)

    def test_failed_previous_service_never_claims_rollback(self):
        result, active, old = self.exercise("dead")
        self.assertEqual(active, old)
        self.assertIn("ROLLBACK_UNVERIFIED:", result.stderr)
        self.assertNotIn("ROLLBACK_VERIFIED:", result.stderr)


if __name__ == "__main__":
    unittest.main()
