"""Exercise the real TOR root-only go-live main SHA guard without network access."""

from pathlib import Path
import json
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
GO_LIVE = (ROOT / "ops/go-live-postgres-vps.sh").read_text(encoding="utf-8")
START = "# TOR_MAIN_SHA_GUARD_BEGIN"
END = "# TOR_MAIN_SHA_GUARD_END"
GOOD_SHA = "a" * 40
OLD_SHA = "b" * 40


def guard_shell():
    if GO_LIVE.count(START) != 1 or GO_LIVE.count(END) != 1:
        raise AssertionError("expected one executable main guard")
    return GO_LIVE.split(START, 1)[1].split(END, 1)[0]


def check_sha(candidate=GOOD_SHA, remote=GOOD_SHA, ref="refs/heads/main", offline=False):
    with tempfile.TemporaryDirectory(prefix="tor-main-sha-") as directory:
        fake_curl = Path(directory) / "curl"
        fake_curl.write_text(
            '#!/usr/bin/env bash\n'
            'if [[ "$TOR_TEST_OFFLINE" == "1" ]]; then exit 7; fi\n'
            'printf "%s\\n" "$TOR_TEST_PAYLOAD"\n',
            encoding="utf-8",
        )
        fake_curl.chmod(0o755)
        env = os.environ.copy()
        env.update({
            "PATH": directory + ":" + env.get("PATH", ""),
            "TOR_TEST_OFFLINE": "1" if offline else "0",
            "TOR_TEST_PAYLOAD": json.dumps({
                "ref": ref,
                "object": {"type": "commit", "sha": remote},
            }),
        })
        script = (
            "set -Eeuo pipefail\n"
            + "sha=" + candidate + "\n"
            + guard_shell()
            + "\nrequire_current_main_sha\n"
        )
        return subprocess.run(
            ["bash", "-c", script],
            capture_output=True,
            text=True,
            timeout=8,
            env=env,
        )


class GoLiveMainShaGuardTests(unittest.TestCase):
    def test_exact_main_passes(self):
        self.assertEqual(check_sha().returncode, 0)

    def test_stale_stage_refused(self):
        result = check_sha(candidate=OLD_SHA)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("REFUSE_OBSOLETE_MAIN", result.stderr)

    def test_mismatched_ref_or_bad_sha_refused(self):
        for remote, ref in ((GOOD_SHA, "refs/heads/other"), ("invalid", "refs/heads/main")):
            with self.subTest(remote=remote, ref=ref):
                self.assertNotEqual(check_sha(remote=remote, ref=ref).returncode, 0)

    def test_unavailable_github_fails_closed(self):
        result = check_sha(offline=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("REFUSE_OBSOLETE_MAIN", result.stderr)

    def test_checks_before_migration_and_before_service_change(self):
        first = GO_LIVE.index("require_current_main_sha\n")
        second = GO_LIVE.index("require_current_main_sha\n", first + 1)
        self.assertLess(first, GO_LIVE.index("migration_script=$(mktemp"))
        self.assertGreater(second, GO_LIVE.index("TOR_SCHEMA_MARKED"))
        self.assertLess(second, GO_LIVE.index('cat > "$dropin"'))
        self.assertIn(
            "https://api.github.com/repos/calex794/tor/git/ref/heads/main",
            guard_shell(),
        )


if __name__ == "__main__":
    unittest.main()
