"""Execute the actual go-live runtime guard with private synthetic files."""
from pathlib import Path
import base64
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SOURCE = (ROOT / "ops/go-live-postgres-vps.sh").read_text()
GUARD = SOURCE.split("# TOR_RUNTIME_ENV_GUARD_BEGIN\n", 1)[1].split(
    "# TOR_RUNTIME_ENV_GUARD_END", 1)[0]
TEMPLATE = (ROOT / "ops/tor-runtime.conf.example").read_bytes()
MASTER = base64.b64encode(b"a" * 32).decode()
V1 = base64.urlsafe_b64encode(b"b" * 32).decode().rstrip("=")
V2 = base64.urlsafe_b64encode(b"c" * 32).decode().rstrip("=")


class RuntimeEnvironmentTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.secret = self.root / "etc-tor"
        self.secret.mkdir(mode=0o700)
        self.dropins = self.root / "dropins"
        self.dropins.mkdir(mode=0o755)
        self.runtime = self.secret / "tor-runtime.env"
        self.dropin = self.dropins / "30-runtime.conf"
        self.db = self.secret / "tor-app.env"
        self.db.write_text("DATABASE_URL=synthetic-db-only\n")
        self.db.chmod(0o600)
        # CI is unprivileged. Exercise real stat modes/link counts but
        # substitute root ownership for these temporary test-owned files.
        self.harness = r'''
stat() {
  local output
  output=$(/usr/bin/stat "$@") || return
  if [[ "$output" == "$(id -un):$(id -gn):"* ]]; then
    printf 'root:root:%s\n' "${output#*:*:}"
  else
    printf '%s\n' "$output"
  fi
}
'''
        self.harness += GUARD + '\nvalidate_runtime_configuration "$1" "$2"\n'

    def install(self, text):
        self.runtime.write_text(text, encoding="ascii")
        self.runtime.chmod(0o600)
        self.dropin.write_bytes(TEMPLATE)
        self.dropin.chmod(0o644)

    def guard(self):
        before = self.db.read_bytes()
        result = subprocess.run(
            ["bash", "-euo", "pipefail", "-c", self.harness, "guard",
             str(self.secret), str(self.dropins)],
            capture_output=True, text=True, timeout=5,
        )
        self.assertEqual(self.db.read_bytes(), before)
        self.assertNotIn(MASTER, result.stdout + result.stderr)
        self.assertNotIn(V1, result.stdout + result.stderr)
        return result

    def test_legacy_database_only_installation(self):
        self.assertEqual(self.guard().returncode, 0)

    def test_separate_master_and_storage_keys_accepted_without_rewrite(self):
        self.install(f"TOR_S3_ADMIN_MASTER_KEY={MASTER}\n"
                     f"TOR_CONTENT_KEK_V1={V1}\nTOR_CONTENT_KEK_V2={V2}\n"
                     "TOR_CONTENT_ACTIVE_KEK_ID=content-kek-v2\n"
                     "AWS_REGION=ru-central1\n"
                     "AWS_ENDPOINT_URL_S3=https://storage.yandexcloud.net\n"
                     "AWS_ACCESS_KEY_ID=synthetic-access\n"
                     "AWS_SECRET_ACCESS_KEY=synthetic-secret-0000\n"
                     "TOR_FREE_S3_BUCKET=tore\nTOR_PAID_GRANTS_ENABLED=0\n")
        original = self.runtime.read_bytes()
        self.assertEqual(self.guard().returncode, 0)
        self.assertEqual(self.guard().returncode, 0)
        self.assertEqual(self.runtime.read_bytes(), original)
        self.assertEqual(self.dropin.read_bytes(), TEMPLATE)

    def test_incomplete_pair_and_bad_permissions(self):
        self.runtime.write_text(f"TOR_S3_ADMIN_MASTER_KEY={MASTER}\n")
        self.runtime.chmod(0o600)
        self.assertNotEqual(self.guard().returncode, 0)
        self.install(f"TOR_S3_ADMIN_MASTER_KEY={MASTER}\n")
        self.runtime.unlink()
        self.assertNotEqual(self.guard().returncode, 0)
        self.install(f"TOR_S3_ADMIN_MASTER_KEY={MASTER}\n")
        self.runtime.chmod(0o644)
        self.assertNotEqual(self.guard().returncode, 0)
        self.runtime.chmod(0o600)
        self.dropin.chmod(0o666)
        self.assertNotEqual(self.guard().returncode, 0)

    def test_never_follow_symlinks_or_accept_modified_dropin(self):
        self.install(f"TOR_S3_ADMIN_MASTER_KEY={MASTER}\n")
        self.runtime.rename(self.secret / "other")
        self.runtime.symlink_to(self.secret / "other")
        self.assertNotEqual(self.guard().returncode, 0)
        self.runtime.unlink()
        self.install(f"TOR_S3_ADMIN_MASTER_KEY={MASTER}\n")
        self.dropin.write_bytes(TEMPLATE + b"Environment=TOR_AUTH_ENABLED=0\n")
        self.assertNotEqual(self.guard().returncode, 0)
        self.dropin.write_bytes(TEMPLATE)
        self.runtime.rename(self.secret / "other-new")
        os.link(self.secret / "other-new", self.runtime)
        self.assertNotEqual(self.guard().returncode, 0)

    def test_refuses_invalid_keys_overrides_and_shell_syntax(self):
        for bad in (
            "TOR_S3_ADMIN_MASTER_KEY=invalid\n",
            f"TOR_S3_ADMIN_MASTER_KEY={MASTER}\nTOR_S3_ADMIN_MASTER_KEY={MASTER}\n",
            f"TOR_CONTENT_KEK_V2={V2}\n",
            f"TOR_CONTENT_KEK_V1={V1}\nTOR_CONTENT_KEK_V2={V1}\n",
            "TOR_CONTENT_ACTIVE_KEK_ID=content-kek-v2\n",
            "TOR_PAID_GRANTS_ENABLED=1\n",
            "DATABASE_URL=other\n", "TOR_AUTH_ENABLED=0\n",
            "TOR_DEMO_MODE=1\n", "TOR_LISTEN_ADDR=0.0.0.0:3000\n",
            "LD_PRELOAD=/tmp/untrusted.so\n",
            "AWS_ACCESS_KEY_ID=synthetic-access\n",
            "AWS_ENDPOINT_URL_S3=http://unsafe.example\n",
            "AWS_REGION=$(touch /tmp/invalid)\n",
            "AWS_REGION=ru-central1\\\n",
            "AWS_REGION=ru-central1\r\n",
        ):
            with self.subTest(variable=bad.split("=")[0]):
                self.install(bad)
                result = self.guard()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("REFUSE_RUNTIME_ENV", result.stderr)

    def test_guard_precedes_migration_and_restart(self):
        self.assertLess(SOURCE.index('validate_runtime_configuration "$secret_dir" /etc'),
                        SOURCE.index('bash "$migration_script" "$sha"'))
        self.assertLess(SOURCE.index('validate_runtime_configuration "$secret_dir" "$dropin_dir"'),
                        SOURCE.index('mv -Tf "$tmp" /opt/tor/current'))
        self.assertIn("EnvironmentFile=/etc/tor/tor-app.env", SOURCE)
        self.assertEqual(TEMPLATE, b"[Service]\nEnvironmentFile=/etc/tor/tor-runtime.env\n")


if __name__ == "__main__":
    unittest.main()
