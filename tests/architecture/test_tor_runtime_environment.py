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
DB_PASSWORD = "d" * 64
DB_LINE = (f"DATABASE_URL=postgresql://tor_app:{DB_PASSWORD}"
           "@127.0.0.1:5432/tor?sslmode=disable")


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
        self.db.write_text(DB_LINE + "\n")
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
        self.harness += GUARD

    def install(self, text):
        self.runtime.write_text(text, encoding="utf-8")
        self.runtime.chmod(0o600)
        self.dropin.write_bytes(TEMPLATE)
        self.dropin.chmod(0o644)

    def guard(self, diagnostic=False):
        before = self.db.read_bytes()
        harness = self.harness + ('\nvalidate_runtime_configuration "$1" "$2" diagnostic\n'
                                  if diagnostic else
                                  '\npassword=$(validate_runtime_configuration "$1" "$2")\n'
                                  '[[ "$password" == "$3" ]]\n')
        result = subprocess.run(
            ["bash", "-euo", "pipefail", "-c", harness, "guard",
             str(self.secret), str(self.dropins), DB_PASSWORD],
            capture_output=True, text=True, timeout=5,
        )
        self.assertEqual(self.db.read_bytes(), before)
        self.assertNotIn(MASTER, result.stdout + result.stderr)
        self.assertNotIn(V1, result.stdout + result.stderr)
        self.assertNotIn(V2, result.stdout + result.stderr)
        self.assertNotIn(DB_PASSWORD, result.stdout + result.stderr)
        self.assertNotIn("synthetic-secret-0000", result.stdout + result.stderr)
        return result

    def test_legacy_database_only_installation(self):
        self.assertEqual(self.guard().returncode, 0)
        self.db.write_text(DB_LINE)
        self.assertEqual(self.guard().returncode, 0)

    def test_previously_documented_combined_app_environment(self):
        # Reproduce the old whole-file regex refusal using the actual legacy
        # configuration shape; retain both DB and application bytes.
        self.db.write_text(
            "# Existing database and Yandex S3 settings\n" + DB_LINE + "\n\n"
            f"TOR_S3_ADMIN_MASTER_KEY={MASTER}\n"
            f"TOR_CONTENT_KEK_V1={V1}\nTOR_CONTENT_KEK_V2={V2}\n"
            "TOR_CONTENT_ACTIVE_KEK_ID=content-kek-v2\n"
            "AWS_REGION=ru-central1\n"
            "AWS_ENDPOINT_URL_S3=https://storage.yandexcloud.net\n"
            "AWS_ACCESS_KEY_ID=synthetic-access\n"
            "AWS_SECRET_ACCESS_KEY=synthetic-secret-0000\n"
            "TOR_FREE_S3_BUCKET=tore\nTOR_FREE_S3_PUBLIC_ORIGIN=https://storage.example.invalid\nTOR_PAID_GRANTS_ENABLED=0\n"
        )
        self.assertEqual(self.guard().returncode, 0)
        self.assertFalse(self.runtime.exists())
        self.assertFalse(self.dropin.exists())

    def test_combined_and_separate_disjoint_settings_are_validated_together(self):
        self.db.write_text(DB_LINE + f"\nTOR_CONTENT_KEK_V1={V1}\n")
        self.install(f"TOR_CONTENT_KEK_V2={V2}\nTOR_CONTENT_ACTIVE_KEK_ID=content-kek-v2\n")
        original = self.runtime.read_bytes()
        self.assertEqual(self.guard().returncode, 0)
        self.assertEqual(self.runtime.read_bytes(), original)
        self.install(f"TOR_CONTENT_KEK_V1={V1}\n")
        self.assertNotEqual(self.guard().returncode, 0)

    def test_combined_file_rejects_unsafe_db_and_runtime_settings(self):
        invalid = (
            "",
            DB_LINE + "\n" + DB_LINE + "\n",
            DB_LINE.replace("tor_app:", "tor_migrator:"),
            DB_LINE.replace("127.0.0.1", "example.com"),
            DB_LINE.replace(":5432", ":5433"),
            DB_LINE.replace("/tor?", "/other?"),
            DB_LINE.replace("sslmode=disable", "sslmode=require"),
            DB_LINE.replace(DB_PASSWORD, "short"),
            DB_LINE + "\nTOR_AUTH_ENABLED=0\n",
            DB_LINE + "\nTOR_DEMO_MODE=1\n",
            DB_LINE + "\nTOR_LISTEN_ADDR=0.0.0.0:3000\n",
            DB_LINE + "\nLD_PRELOAD=/tmp/untrusted.so\n",
            DB_LINE + "\nAWS_REGION=$(id)\n",
            DB_LINE + "\nTOR_S3_ADMIN_MASTER_KEY=invalid\n",
            DB_LINE + "\nAWS_ACCESS_KEY_ID=synthetic-access\n",
            DB_LINE + f"\nTOR_CONTENT_KEK_V2={V2}\n",
            DB_LINE + "\nAWS_REGION=ru-central1\nAWS_REGION=ru-central1\n",
        )
        for index, content in enumerate(invalid):
            with self.subTest(index=index):
                self.db.write_text(content)
                result = self.guard()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("REFUSE_RUNTIME_ENV", result.stderr)

    def test_combined_file_rejects_unsafe_permissions_and_links(self):
        self.db.chmod(0o644)
        self.assertNotEqual(self.guard().returncode, 0)
        self.db.chmod(0o600)
        other = self.secret / "copy"
        os.link(self.db, other)
        self.assertNotEqual(self.guard().returncode, 0)
        self.db.unlink()
        self.db.symlink_to(other)
        self.assertNotEqual(self.guard().returncode, 0)

    def test_separate_master_and_storage_keys_accepted_without_rewrite(self):
        self.install(f"TOR_S3_ADMIN_MASTER_KEY={MASTER}\n"
                     f"TOR_CONTENT_KEK_V1={V1}\nTOR_CONTENT_KEK_V2={V2}\n"
                     "TOR_CONTENT_ACTIVE_KEK_ID=content-kek-v2\n"
                     "AWS_REGION=ru-central1\n"
                     "AWS_ENDPOINT_URL_S3=https://storage.yandexcloud.net\n"
                     "AWS_ACCESS_KEY_ID=synthetic-access\n"
                     "AWS_SECRET_ACCESS_KEY=synthetic-secret-0000\n"
                     "TOR_FREE_S3_BUCKET=tore\nTOR_FREE_S3_PUBLIC_ORIGIN=https://storage.example.invalid\nTOR_PAID_GRANTS_ENABLED=0\n")
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

    def test_diagnostics_report_multiple_failures_without_secret_output(self):
        unknown = "PRIVATE_UNKNOWN_IDENTIFIER_ABC"
        self.db.write_text(
            DB_LINE + "\n" + unknown + "=private-unknown-value\n"
            "AWS_REGION=UPPERCASE\n"
            "TOR_AUTH_ENABLED=0\n"
            "AWS_REGION=ru-central1\n"
            "TOR_S3_ADMIN_MASTER_KEY=private-malformed-key\n"
            "AWS_ACCESS_KEY_ID=synthetic-access\n"
        )
        result = self.guard(diagnostic=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        for expected in (
            "file=tor-app.env line=2 parameter=unknown reason=unsupported-parameter",
            "file=tor-app.env line=3 parameter=AWS_REGION reason=invalid-region",
            "file=tor-app.env line=4 parameter=TOR_AUTH_ENABLED reason=unsupported-parameter",
            "file=tor-app.env line=5 parameter=AWS_REGION reason=duplicate-parameter",
            "file=tor-app.env line=6 parameter=TOR_S3_ADMIN_MASTER_KEY reason=invalid-key-encoding",
            "file=tor-app.env line=7 parameter=AWS_ACCESS_KEY_ID reason=incomplete-aws-credential-pair",
        ):
            self.assertIn(expected, result.stderr)
        for secret in (unknown, "private-unknown-value", "UPPERCASE", "private-malformed-key"):
            self.assertNotIn(secret, result.stderr)
        self.assertNotEqual(self.guard().returncode, 0)

    def test_utf8_operator_comments_pass_same_release_guard(self):
        # Production refusal reproduced: the first non-ASCII byte occurs
        # on line 15, but belongs to an ignored operator comment.
        app = DB_LINE + "\n" + "\n" * 13 + "# Настройки хранилища\n"
        self.db.write_text(app, encoding="utf-8")
        self.install(" # Комментарий оператора\nAWS_REGION=ru-central1\n")
        original_app = self.db.read_bytes()
        original_runtime = self.runtime.read_bytes()
        result = self.guard(diagnostic=True)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "TOR_RUNTIME_ENV_OK\n")
        self.assertEqual(result.stderr, "")
        self.assertEqual(self.guard().returncode, 0)
        self.assertEqual(self.db.read_bytes(), original_app)
        self.assertEqual(self.runtime.read_bytes(), original_runtime)

    def test_utf8_support_does_not_relax_assignment_validation(self):
        cases = (
            ("AWS_REGION=регион\n", "parameter=AWS_REGION reason=invalid-value-syntax"),
            ("ПАРАМЕТР=value\n", "parameter=unknown reason=invalid-assignment"),
            ("# комментарий\x00\n", "parameter=unknown reason=control-character"),
        )
        for suffix, expected in cases:
            with self.subTest(expected=expected):
                self.db.write_text(DB_LINE + "\n" + suffix, encoding="utf-8")
                result = self.guard(diagnostic=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(expected, result.stderr)
                self.assertNotEqual(self.guard().returncode, 0)
        self.db.write_bytes(DB_LINE.encode("ascii") + b"\n# \xff\n")
        result = self.guard(diagnostic=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("file=tor-app.env line=2 parameter=unknown reason=invalid-utf8-file",
                      result.stderr)
        self.assertNotEqual(self.guard().returncode, 0)

    def test_free_bucket_requires_verified_origin_before_sqlx(self):
        self.db.write_text(DB_LINE + "\nTOR_FREE_S3_BUCKET=tore\n")
        result = self.guard(diagnostic=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("parameter=TOR_FREE_S3_BUCKET reason=missing-TOR_FREE_S3_PUBLIC_ORIGIN",
                      result.stderr)
        self.assertNotEqual(self.guard().returncode, 0)
        self.install("TOR_FREE_S3_PUBLIC_ORIGIN=https://storage.example.invalid\n")
        self.assertEqual(self.guard(diagnostic=True).stdout, "TOR_RUNTIME_ENV_OK\n")
        self.assertEqual(self.guard().returncode, 0)

    def test_paid_grants_require_bucket_origin_and_key_before_sqlx(self):
        self.db.write_text(DB_LINE + "\nTOR_PAID_GRANTS_ENABLED=1\n")
        result = self.guard(diagnostic=True)
        self.assertNotEqual(result.returncode, 0)
        for required in ("TOR_CONTENT_KEK_V1", "TOR_PAID_S3_BUCKET", "TOR_PAID_S3_PUBLIC_ORIGIN"):
            self.assertIn("parameter=TOR_PAID_GRANTS_ENABLED reason=missing-" + required,
                          result.stderr)
        self.install(f"TOR_CONTENT_KEK_V1={V1}\n"
                     "TOR_PAID_S3_BUCKET=paid-bucket\n"
                     "TOR_PAID_S3_PUBLIC_ORIGIN=https://storage.example.invalid\n")
        self.assertEqual(self.guard().returncode, 0)
        self.install("TOR_PAID_AUTHOR_S3_ONLY_ENABLED=1\n")
        self.assertIn("parameter=TOR_PAID_AUTHOR_S3_ONLY_ENABLED reason=paid-author-not-enabled",
                      self.guard(diagnostic=True).stderr)

    def test_diagnostics_report_invalid_database_without_echoing_url(self):
        invalid = DB_LINE.replace("127.0.0.1", "private-host.invalid")
        self.db.write_text(invalid + "\n")
        result = self.guard(diagnostic=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("parameter=DATABASE_URL reason=invalid-local-database-url", result.stderr)
        self.assertNotIn("private-host.invalid", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_diagnostics_for_split_duplicates_and_valid_configuration(self):
        self.db.write_text(DB_LINE + "\nAWS_REGION=ru-central1\n")
        self.install("AWS_REGION=ru-central1\n")
        original = self.runtime.read_bytes()
        result = self.guard(diagnostic=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("file=tor-runtime.env line=1 parameter=AWS_REGION reason=duplicate-across-files",
                      result.stderr)
        self.assertEqual(self.runtime.read_bytes(), original)
        self.install(f"TOR_S3_ADMIN_MASTER_KEY={MASTER}\n")
        original = self.runtime.read_bytes()
        result = self.guard(diagnostic=True)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "TOR_RUNTIME_ENV_OK\n")
        self.assertEqual(result.stderr, "")
        self.assertEqual(self.runtime.read_bytes(), original)
        self.assertEqual(self.dropin.read_bytes(), TEMPLATE)
        self.assertEqual(self.guard().returncode, 0)

    def test_diagnostic_entry_point_is_root_only_and_never_falls_through(self):
        block = SOURCE.split("# TOR_RUNTIME_ENV_DIAGNOSTIC_BEGIN\n", 1)[1].split(
            "# TOR_RUNTIME_ENV_DIAGNOSTIC_END", 1)[0]
        self.assertLess(SOURCE.index("# TOR_RUNTIME_ENV_DIAGNOSTIC_BEGIN"),
                        SOURCE.index("require_current_main_sha\n"))
        for uid, guard_status, expected in ((1000, 0, 2), (0, 0, 0), (0, 1, 1)):
            with self.subTest(uid=uid, guard_status=guard_status):
                harness = f"id() {{ echo {uid}; }}\n" + (
                    'validate_runtime_configuration() {\n'
                    '  [[ "$*" == "/etc/tor /etc/systemd/system/tor-web.service.d diagnostic" ]] || exit 98\n'
                    '  echo "GUARD_CALLED"\n'
                    f'  return {guard_status}\n'
                    '}\n'
                ) + block + "\nexit 99\n"
                result = subprocess.run(["bash", "-euo", "pipefail", "-c", harness,
                                         "go-live", "--check-runtime"],
                                        capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, expected)
                self.assertEqual("GUARD_CALLED" in result.stdout, uid == 0)

    def test_guard_precedes_migration_and_restart(self):
        self.assertLess(SOURCE.index('validate_runtime_configuration "$secret_dir" /etc'),
                        SOURCE.index('bash "$migration_script" "$sha"'))
        self.assertLess(SOURCE.index('validate_runtime_configuration "$secret_dir" "$dropin_dir"'),
                        SOURCE.index('mv -Tf "$tmp" /opt/tor/current'))
        self.assertIn("EnvironmentFile=/etc/tor/tor-app.env", SOURCE)
        self.assertIn('PGPASSWORD="$app_password" psql -X -w', SOURCE)
        self.assertLess(SOURCE.index('PGPASSWORD="$app_password" psql'),
                        SOURCE.index('bash "$migration_script" "$sha"'))
        self.assertIn('validate_runtime_configuration "$secret_dir" "$dropin_dir" >/dev/null',
                      SOURCE)
        self.assertNotIn('app_line=$(cat "$app_secret")', SOURCE)
        self.assertEqual(TEMPLATE, b"[Service]\nEnvironmentFile=/etc/tor/tor-runtime.env\n")


if __name__ == "__main__":
    unittest.main()
