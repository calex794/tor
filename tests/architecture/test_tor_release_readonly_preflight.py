"""Read-only TOR VPS preflight: run real shell with fake services and GitHub."""

from pathlib import Path
import hashlib
import json
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SOURCE = (ROOT / "ops/check-tor-release-vps.sh").read_text(encoding="utf-8")
OLD_SHA = "a" * 40
NEW_SHA = "b" * 40
MIGRATIONS = [
    *range(202610120001, 202610120007),
    *range(202610130001, 202610130004),
    202610260001,
    202610260002,
    202610260004,
    202610260005,
    202610260006,
    202610260007,
    202610260008,
    *range(202610270001, 202610270011),
    202610280001,
    202610280002,
]


def executable(path: Path, body: str) -> None:
    path.write_text(body, encoding="utf-8")
    path.chmod(0o755)


class ReadOnlyVpsPreflightTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="tor-vps-preflight-")
        base = Path(self.tmp.name)
        self.tor = base / "opt/tor"
        self.releases = self.tor / "releases"
        self.marker_dir = base / "var/lib/tor-release"
        self.marker_dir.mkdir(parents=True)
        self.releases.mkdir(parents=True)
        self.old = self.releases / OLD_SHA
        self.new = self.releases / NEW_SHA
        for folder in (self.old, self.new):
            folder.mkdir()
            executable(folder / "tor-web", "#!/bin/sh\nexit 0\n")
            executable(folder / "tor-migrate", "#!/bin/sh\nexit 0\n")
            digest = hashlib.sha256((folder / "tor-migrate").read_bytes()).hexdigest()
            (folder / "tor-migrate.sha256").write_text(
                digest + "  tor-migrate\n", encoding="ascii"
            )
        self.current = self.tor / "current"
        self.current.symlink_to(self.old)
        self.schema = self.marker_dir / "schema-version"
        self.schema.write_text("202610260007\n", encoding="ascii")
        (self.marker_dir / "pg-activated").write_text("PG_ACTIVATED\n", encoding="ascii")

        self.fakebin = base / "bin"
        self.fakebin.mkdir()
        executable(self.fakebin / "systemctl", '#!/bin/sh\n'
                   'test "$TOR_TEST_SERVICE" = active\n')
        executable(self.fakebin / "runuser", '#!/bin/sh\n'
                   'printf "%s\\n" "$TOR_TEST_HISTORY"\n')
        executable(self.fakebin / "curl", """#!/usr/bin/env python3
import json
import os
import sys
url = sys.argv[-1]
if os.environ.get("TOR_TEST_NETWORK") == "offline":
    sys.exit(7)
if url.endswith("/readyz"):
    print("ok")
elif "/git/ref/heads/main" in url:
    print(json.dumps({"ref": os.environ.get("TOR_TEST_REF", "refs/heads/main"),
                      "object": {"sha": os.environ["TOR_TEST_MAIN"]}}))
elif "/contents/migrations?" in url:
    print(os.environ["TOR_TEST_MANIFEST"])
else:
    sys.exit(8)
""")
        self.script = base / "check.sh"
        content = SOURCE.replace("/opt/tor", str(self.tor)).replace(
            "/var/lib/tor-release", str(self.marker_dir)
        )
        content = content.replace("if [[ $(id -u) != 0 ]]; then", "if false; then")
        executable(self.script, content)
        syntax = subprocess.run(
            ["bash", "-n", str(self.script)], capture_output=True, text=True
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr)
        self.env = os.environ.copy()
        self.env.update({
            "PATH": str(self.fakebin) + ":" + self.env.get("PATH", ""),
            "TOR_TEST_MAIN": NEW_SHA,
            "TOR_TEST_NETWORK": "online",
            "TOR_TEST_SERVICE": "active",
            "TOR_TEST_HISTORY": "15:202610260007:0",
            "TOR_TEST_MANIFEST": json.dumps([
                {"name": str(v) + "_test.sql", "type": "file"}
                for v in MIGRATIONS
            ]),
        })

    def tearDown(self):
        self.tmp.cleanup()

    def run_preflight(self, candidate=NEW_SHA):
        return subprocess.run(
            ["bash", str(self.script), candidate],
            capture_output=True,
            text=True,
            timeout=12,
            env=self.env,
        )

    def test_read_only_success_reports_pending_migrations(self):
        previous = self.current.resolve()
        result = self.run_preflight()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("TOR_MIGRATIONS_PENDING 13", result.stdout)
        self.assertIn("TOR_PREFLIGHT_STAGE_OK_MIGRATIONS_REQUIRED", result.stdout)
        self.assertEqual(self.current.resolve(), previous)
        self.assertEqual(self.schema.read_text(), "202610260007\n")

    def test_stale_candidate_is_rejected(self):
        self.env["TOR_TEST_MAIN"] = OLD_SHA
        result = self.run_preflight()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not the current GitHub main", result.stderr)

    def test_offline_github_is_rejected(self):
        self.env["TOR_TEST_NETWORK"] = "offline"
        result = self.run_preflight()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cannot verify GitHub main", result.stderr)

    def test_corrupt_migrator_is_rejected(self):
        (self.new / "tor-migrate").write_text("modified\n")
        result = self.run_preflight()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum mismatch", result.stderr)

    def test_schema_marker_and_sqlx_history_disagreement_is_rejected(self):
        self.env["TOR_TEST_HISTORY"] = "15:202610260006:0"
        result = self.run_preflight()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("disagree", result.stderr)

    def test_unhealthy_current_server_is_rejected(self):
        self.env["TOR_TEST_SERVICE"] = "inactive"
        result = self.run_preflight()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not active", result.stderr)

    def test_missing_candidate_and_malformed_manifest_are_rejected(self):
        self.assertNotEqual(self.run_preflight("c" * 40).returncode, 0)
        self.env["TOR_TEST_MANIFEST"] = '{"message":"rate limit"}'
        self.assertIn("cannot verify canonical SQLx", self.run_preflight().stderr)

    def test_bash_script_contains_no_mutating_release_actions(self):
        for forbidden in (
            "systemctl restart",
            "systemctl stop",
            "ln -s",
            "mv -Tf",
            "pg_restore -d",
            "tor-migrate\n",
            "source /etc/tor/",
        ):
            self.assertNotIn(forbidden, SOURCE)
        self.assertIn("TOR_PREFLIGHT_READ_ONLY", SOURCE)


if __name__ == "__main__":
    unittest.main()
