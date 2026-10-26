"""Exercise the exact TOR VPS activation shell with fake services and no real VPS."""
from pathlib import Path
import os
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[2]
STAGE = (ROOT / ".github/workflows/tor-vps-staging.yml").read_text(encoding="utf-8")
PREVIOUS_SHA = "a" * 40
CANDIDATE_SHA = "b" * 40


def activation_script() -> str:
    """Extract the real SSH heredoc, not a separately invented simulation."""
    start = "<<'REMOTE'\n"
    if STAGE.count(start) != 1:
        raise AssertionError("expected exactly one deployment SSH heredoc")
    tail = STAGE.split(start, 1)[1]
    if "\n          REMOTE" not in tail:
        raise AssertionError("missing deployment SSH heredoc terminator")
    return textwrap.dedent(tail.split("\n          REMOTE", 1)[0]) + "\n"


def executable(path: Path, body: str):
    path.write_text(body, encoding="utf-8")
    path.chmod(0o755)


class ActivationRollbackTests(unittest.TestCase):
    def run_candidate(self, mode: str):
        with tempfile.TemporaryDirectory(prefix="tor-release-rollback-") as temp:
            root = Path(temp) / "tor"
            prior = root / "releases" / PREVIOUS_SHA
            candidate = root / "releases" / CANDIDATE_SHA
            prior.mkdir(parents=True)
            candidate.mkdir(parents=True)
            executable(prior / "tor-web", "#!/bin/sh\nexit 0\n")
            executable(candidate / "tor-web", "#!/bin/sh\nexit 0\n")
            current = root / "current"
            current.symlink_to(prior)
            switched = Path(temp) / "switched"
            fake_bin = Path(temp) / "bin"
            fake_bin.mkdir()
            executable(
                fake_bin / "systemctl",
                """#!/usr/bin/env bash
set -eu
case "$1" in
  is-active) exit 0 ;;
  show) printf 'ActiveState=failed\\nResult=exit-code\\n'; exit 0 ;;
  restart) touch "$TOR_TEST_SWITCHED"; exit 0 ;;
  stop) exit 0 ;;
esac
exit 90
""",
            )
            executable(
                fake_bin / "sudo",
                """#!/usr/bin/env bash
set -eu
[[ "$1" == -n ]] && shift
[[ "$1" == /usr/bin/systemctl ]] || exit 91
shift
exec systemctl "$@"
""",
            )
            executable(
                fake_bin / "curl",
                """#!/usr/bin/env bash
set -eu
for url in "$@"; do :; done
target="$(readlink "$TOR_TEST_CURRENT" || true)"
if [[ "$target" == "$TOR_TEST_CANDIDATE_PATH" ]]; then
  [[ "$TOR_TEST_MODE" == success ]] || exit 7
elif [[ "$TOR_TEST_MODE" == preflight_dead ||
        ( "$TOR_TEST_MODE" == rollback_dead && -f "$TOR_TEST_SWITCHED" ) ]]; then
  exit 7
fi
case "$url" in
  */readyz) printf 'ok\\n' ;;
  */register) printf '<form><input name="csrf_token" value="mock"></form>\\n' ;;
  *) exit 8 ;;
esac
""",
            )
            shell = activation_script().replace("/opt/tor", str(root))
            # The test exercises state transitions without 35 real sleeps.
            for count in ("20", "15"):
                shell = shell.replace(
                    "for i in $(seq 1 " + count + ")",
                    "for i in $(seq 1 1)",
                )
            shell = shell.replace("sleep 1", ":")
            script_path = Path(temp) / "activate.sh"
            executable(script_path, shell)
            syntax = subprocess.run(["bash", "-n", str(script_path)], capture_output=True, text=True)
            self.assertEqual(syntax.returncode, 0, syntax.stderr)
            env = os.environ.copy()
            env.update({
                "PATH": str(fake_bin) + ":" + os.environ.get("PATH", ""),
                "TOR_TEST_CURRENT": str(current),
                "TOR_TEST_CANDIDATE_PATH": str(candidate),
                "TOR_TEST_SWITCHED": str(switched),
                "TOR_TEST_MODE": mode,
            })
            result = subprocess.run(
                ["bash", str(script_path), CANDIDATE_SHA],
                capture_output=True,
                text=True,
                env=env,
                timeout=10,
            )
            return result, current.resolve(), prior, candidate

    def test_healthy_candidate_switches(self):
        run, target, old, candidate = self.run_candidate("success")
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(target, candidate)
        self.assertIn("Active tor commit: " + CANDIDATE_SHA, run.stdout)

    def test_failed_candidate_restores_and_verifies_previous_http(self):
        run, target, old, candidate = self.run_candidate("candidate_dead")
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual(target, old)
        self.assertIn("ACTIVATION_FAILED:", run.stderr)
        self.assertIn("ROLLBACK_VERIFIED:", run.stderr)
        self.assertNotIn("ROLLBACK_UNVERIFIED:", run.stderr)

    def test_failed_rollback_never_claims_success(self):
        run, target, old, candidate = self.run_candidate("rollback_dead")
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual(target, old)
        self.assertIn("ROLLBACK_UNVERIFIED:", run.stderr)
        self.assertNotIn("ROLLBACK_VERIFIED:", run.stderr)

    def test_unhealthy_previous_server_blocks_automatic_switch(self):
        run, target, old, candidate = self.run_candidate("preflight_dead")
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual(target, old)
        self.assertIn("REFUSING_AUTO_ACTIVATION:", run.stderr)
        self.assertNotIn("ACTIVATION_FAILED:", run.stderr)


if __name__ == "__main__":
    unittest.main()
