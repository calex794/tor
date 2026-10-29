"""I-11: run actual TOR operator rollback shell in a temporary fake host."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SOURCE = (ROOT / "ops/verify-tor-rollback.sh").read_text(encoding="utf-8")
PREVIOUS = "a" * 40
CURRENT = "b" * 40
HOST = "tor-stage.example.test"


def executable(path, body):
    path.write_text(body, encoding="utf-8")
    path.chmod(0o755)


class OperatorRollbackDrillTests(unittest.TestCase):
    def exercise(self, mode="success", hostname=HOST, symlink_previous=False,
                 missing_previous=False, default_host=False):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            root = base / "opt/tor"
            previous = root / "releases" / PREVIOUS
            current = root / "releases" / CURRENT
            current.mkdir(parents=True)
            executable(current / "tor-web", "#!/bin/sh\nexit 0\n")
            if symlink_previous:
                external = base / "external"
                external.mkdir()
                executable(external / "tor-web", "#!/bin/sh\nexit 0\n")
                previous.symlink_to(external)
            elif not missing_previous:
                previous.mkdir()
                executable(previous / "tor-web", "#!/bin/sh\nexit 0\n")
            link = root / "current"
            link.symlink_to(current)

            count = base / "restarts"
            count.write_text("0\n", encoding="ascii")
            curl_log = base / "curl.log"
            curl_log.write_text("", encoding="utf-8")
            fakebin = base / "fakebin"
            fakebin.mkdir()
            executable(fakebin / "systemctl", r"""#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  is-active) exit 0 ;;
  restart)
    n=$(cat "$TOR_TEST_COUNT")
    printf '%s\n' "$((n + 1))" > "$TOR_TEST_COUNT"
    exit 0 ;;
esac
exit 70
""")
            executable(fakebin / "curl", r"""#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$TOR_TEST_CURL_LOG"
for url in "$@"; do :; done
release=$(readlink -f "$TOR_TEST_CURRENT")
n=$(cat "$TOR_TEST_COUNT")
if [[ "$TOR_TEST_MODE" == preflight_dead && "$release" == "$TOR_TEST_ACTIVE" && "$n" == 0 ]]; then
  exit 7
fi
if [[ "$TOR_TEST_MODE" == previous_dead && "$release" == "$TOR_TEST_PREVIOUS" ]]; then
  exit 7
fi
if [[ "$TOR_TEST_MODE" == previous_https_dead && "$release" == "$TOR_TEST_PREVIOUS" && "$url" == https://* ]]; then
  exit 7
fi
if [[ "$TOR_TEST_MODE" == restore_dead && "$release" == "$TOR_TEST_ACTIVE" && "$n" -ge 2 ]]; then
  exit 7
fi
if [[ "$url" == */readyz ]]; then
  printf 'ok\n'
  exit 0
fi
if [[ "$url" == "https://$TOR_TEST_HOST/catalog" && " $* " == *" --resolve $TOR_TEST_HOST:443:127.0.0.1 "* ]]; then
  exit 0
fi
exit 8
""")
            source = SOURCE.replace("/opt/tor", str(root))
            source = source.replace('if [[ "$(id -u)" -ne 0 ]]; then', 'if false; then')
            source = source.replace("seq 1 20", "seq 1 1").replace("sleep 1", ":")
            script = base / "rollback.sh"
            executable(script, source)
            syntax = subprocess.run(["bash", "-n", str(script)], capture_output=True, text=True)
            self.assertEqual(syntax.returncode, 0, syntax.stderr)
            env = os.environ.copy()
            env.update({
                "PATH": str(fakebin) + ":" + env.get("PATH", ""),
                "TOR_TEST_CURRENT": str(link),
                "TOR_TEST_ACTIVE": str(current),
                "TOR_TEST_PREVIOUS": str(previous),
                "TOR_TEST_COUNT": str(count),
                "TOR_TEST_CURL_LOG": str(curl_log),
                "TOR_TEST_MODE": mode,
                "TOR_TEST_HOST": "pandaolate.ru" if default_host else hostname,
            })
            if default_host:
                env.pop("TOR_ROLLBACK_HOST", None)
            else:
                env["TOR_ROLLBACK_HOST"] = hostname
            result = subprocess.run(
                ["bash", str(script), PREVIOUS], capture_output=True,
                text=True, env=env, timeout=12,
            )
            return result, link.resolve(), current, int(count.read_text()), curl_log.read_text()

    def test_staging_rollback_and_restore_both_healthy(self):
        run, active, original, restarts, log = self.exercise()
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(active, original)
        self.assertEqual(restarts, 2)
        for marker in ("ROLLBACK_OK", "CURRENT_OK", "RESTORE_OK"):
            self.assertIn(marker, run.stdout)
        self.assertIn("--resolve " + HOST + ":443:127.0.0.1", log)
        self.assertIn("https://" + HOST + "/catalog", log)
        self.assertNotIn("pandaolate.ru", log)

    def test_default_host_remains_compatible(self):
        run, active, original, restarts, log = self.exercise(default_host=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(active, original)
        self.assertIn("--resolve pandaolate.ru:443:127.0.0.1", log)

    def test_failed_previous_readiness_restores_original(self):
        run, active, original, restarts, log = self.exercise(mode="previous_dead")
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual(active, original)
        self.assertEqual(restarts, 2)
        self.assertIn("RESTORE_OK", run.stdout)
        self.assertNotIn("ROLLBACK_OK", run.stdout)

    def test_failed_previous_https_restores_original(self):
        run, active, original, restarts, log = self.exercise(mode="previous_https_dead")
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual(active, original)
        self.assertEqual(restarts, 2)
        self.assertIn("RESTORE_OK", run.stdout)

    def test_failed_restore_never_claims_recovery(self):
        run, active, original, restarts, log = self.exercise(mode="restore_dead")
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual(active, original)
        self.assertEqual(restarts, 2)
        self.assertIn("RESTORE FAILED HEALTH CHECK", run.stderr)
        self.assertNotIn("RESTORE_OK", run.stdout)

    def test_unhealthy_current_is_never_restarted(self):
        run, active, original, restarts, log = self.exercise(mode="preflight_dead")
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual(active, original)
        self.assertEqual(restarts, 0)

    def test_symlinked_or_missing_previous_is_rejected(self):
        for variant in ({"symlink_previous": True}, {"missing_previous": True}):
            with self.subTest(variant=variant):
                run, active, original, restarts, log = self.exercise(**variant)
                self.assertNotEqual(run.returncode, 0)
                self.assertEqual(active, original)
                self.assertEqual(restarts, 0)

    def test_unsafe_host_is_rejected_without_touching_services(self):
        for hostname in ("bad..host", "host;echo pwned", "-evil.example",
                         "https://host", "x" * 254):
            with self.subTest(hostname=hostname):
                run, active, original, restarts, log = self.exercise(hostname=hostname)
                self.assertNotEqual(run.returncode, 0)
                self.assertEqual(active, original)
                self.assertEqual(restarts, 0)
                self.assertIn("unsafe TLS hostname", run.stderr)


if __name__ == "__main__":
    unittest.main()
