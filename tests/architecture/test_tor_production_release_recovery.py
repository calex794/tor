"""Production profile and one-shot root approval: executable contract tests."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
HELPER = (ROOT / "ops/one-click-release-vps.sh").read_text()
GO_LIVE = (ROOT / "ops/go-live-postgres-vps.sh").read_text()
MIGRATE = (ROOT / "ops/apply-tor-migrations-vps.sh").read_text()
WORKFLOW = (ROOT / ".github/workflows/tor-one-click-release.yml").read_text()
BUILD = ROOT / "tools/build_tor_production_web.sh"
PROFILE = "TOR_RELEASE_CAPABILITIES s3-covers=true s3-free=true s3-paid=true s3-chapter-import=true"
SHA = "b" * 40


class ProductionReleaseRecoveryTests(unittest.TestCase):
    def test_profile_is_shared_by_required_ci_and_manual_release(self):
        self.assertIn("bash tools/build_tor_production_web.sh", WORKFLOW)
        for name in ("tor-integration.yml", "tor-cover-s3.yml"):
            source = (ROOT / ".github/workflows" / name).read_text()
            job = source.split("  s3-cover-build:\n", 1)[1]
            self.assertIn("bash tools/build_tor_production_web.sh", job)
            self.assertIn("bash tools/test_tor_production_web_startup.sh", job)
        self.assertIn(PROFILE, BUILD.read_text())
        self.assertIn(PROFILE, GO_LIVE)
        self.assertLess(GO_LIVE.index("--release-capabilities"),
                        GO_LIVE.index('bash "$migration_script" "$sha"'))
        self.assertIn("runuser -u tor-deploy -- env -i", GO_LIVE)

    def test_build_gate_uses_actual_binary_capabilities(self):
        for supported in (True, False):
            with self.subTest(supported=supported), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                bin_dir = root / "bin"
                bin_dir.mkdir()
                cargo = bin_dir / "cargo"
                cargo.write_text(
                    "#!/bin/bash\n"
                    "[[ \"$*\" == \"build --locked --release -p tor-web --features "
                    "s3-covers,s3-free,s3-paid,s3-chapter-import\" ]] || exit 2\n"
                    "mkdir -p target/release\n"
                    "cat > target/release/tor-web <<'EOF'\n"
                    "#!/bin/bash\n"
                    "[[ \"$1\" == --release-capabilities ]] || exit 2\n"
                    "printf '%s\\n' '" + (
                        PROFILE if supported else PROFILE.replace("s3-covers=true", "s3-covers=false")
                    ) + "'\nEOF\nchmod +x target/release/tor-web\n"
                )
                cargo.chmod(0o755)
                result = subprocess.run(
                    ["bash", str(BUILD)], cwd=root,
                    env=dict(os.environ, PATH=str(bin_dir) + ":" + os.environ["PATH"]),
                    capture_output=True, text=True, timeout=5,
                )
                self.assertEqual(result.returncode, 0 if supported else 1, result.stderr)
                if supported:
                    self.assertEqual(result.stdout.strip(), PROFILE)
                else:
                    self.assertIn("lacks required compiled capabilities", result.stderr)

    def test_root_approval_exact_sha_permissions_and_single_link(self):
        source = HELPER.split("# TOR_RECOVERY_APPROVAL_BEGIN\n", 1)[1].split(
            "# TOR_RECOVERY_APPROVAL_END", 1)[0]
        # Substitute ownership only, while checking real modes, links and data.
        harness = r'''
stat() {
  local output
  output=$(/usr/bin/stat "$@") || return
  if [[ "$output" == "$(id -un):$(id -gn):"* ]]; then
    printf 'root:root:%s\n' "${output#*:*:}"
  else
    printf '%s\n' "$output"
  fi
}
''' + source + '\nrequire_recovery_approval "$1" "$2"\n'
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp) / "etc-tor"
            directory.mkdir(mode=0o700)
            marker = directory / "tor-release-recovery.sha"

            def approved():
                return subprocess.run(
                    ["bash", "-euo", "pipefail", "-c", harness, "test",
                     str(marker), SHA], capture_output=True, timeout=5,
                ).returncode == 0

            self.assertFalse(approved())
            marker.write_text("a" * 40 + "\n")
            marker.chmod(0o600)
            self.assertFalse(approved())
            marker.write_text(SHA + "\n\n")
            self.assertFalse(approved())
            marker.write_text(SHA + "\n")
            self.assertTrue(approved())
            marker.chmod(0o644)
            self.assertFalse(approved())
            marker.chmod(0o600)
            linked = directory / "linked"
            os.link(marker, linked)
            self.assertFalse(approved())
            marker.unlink()
            marker.symlink_to(linked)
            self.assertFalse(approved())

    def test_recovery_does_not_bypass_signature_backup_or_database_roles(self):
        self.assertIn("[[ $# == 1 && $1 =~ ^[0-9a-f]{40}$ ]]", HELPER)
        self.assertIn('approval=/etc/tor/tor-release-recovery.sha', HELPER)
        self.assertLess(HELPER.index("root-owned release signature is invalid"),
                        HELPER.index('rm -f -- "$approval"'))
        self.assertLess(HELPER.index('rm -f -- "$approval"'),
                        HELPER.index('bash "$runner" "$sha" --recover'))
        self.assertIn("exact-SHA root recovery approval required", HELPER)
        self.assertIn("Refusing recovery without verified previous PostgreSQL activation", GO_LIVE)
        self.assertLess(GO_LIVE.index('PGPASSWORD="$app_password" psql'),
                        GO_LIVE.index('bash "$migration_script" "$sha" --recover'))
        self.assertIn("pg_dump -Fc --no-owner --no-privileges", MIGRATE)
        self.assertIn('pg_restore -l "$backup"', MIGRATE)
        self.assertIn("tor_app can SELECT but cannot modify migration history", MIGRATE)
        self.assertIn("prior PostgreSQL activation and stopped web required", MIGRATE)
        self.assertIn("TOR_RECOVERY_SCHEMA_READY", MIGRATE)

    def test_stage_only_does_not_activate(self):
        self.assertIn("stage_only:\n", WORKFLOW)
        self.assertIn("default: false", WORKFLOW)
        self.assertEqual(WORKFLOW.count("if: ${{ !inputs.stage_only }}"), 2)
        self.assertIn("if: ${{ inputs.stage_only }}", WORKFLOW)
        self.assertIn("TOR_STAGE_ONLY_OK", WORKFLOW)
        self.assertLess(WORKFLOW.index("TOR_RELEASE_STAGED"),
                        WORKFLOW.index("TOR_STAGE_ONLY_OK"))

    def test_modified_shell_scripts_parse(self):
        for path in ("ops/one-click-release-vps.sh", "ops/go-live-postgres-vps.sh",
                     "ops/apply-tor-migrations-vps.sh", "tools/build_tor_production_web.sh",
                     "tools/test_tor_production_web_startup.sh"):
            with self.subTest(path=path):
                result = subprocess.run(["bash", "-n", str(ROOT / path)],
                                        capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
