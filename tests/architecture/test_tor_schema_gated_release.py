"""Refuse schema-blind TOR VPS activations even when build checks are green."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
STAGE = (ROOT / ".github/workflows/tor-vps-staging.yml").read_text(encoding="utf-8")
GO_LIVE = (ROOT / "ops/go-live-postgres-vps.sh").read_text(encoding="utf-8")


def activation_has_schema_guard(workflow: str) -> bool:
    for name in (
        "Refuse obsolete main release before switching active binary",
        "Activate verified release only when explicitly permitted",
    ):
        match = re.search(
            r"(?m)^\s*- name: " + re.escape(name) + r"\n\s*if: ([^\n]+)",
            workflow,
        )
        if not match or "steps.schema_guard.outputs.can_activate == 'true'" not in match.group(1):
            return False
    return True


class SchemaGatedReleaseTests(unittest.TestCase):
    def test_auto_switch_requires_root_verified_schema(self):
        self.assertIn("id: schema_guard", STAGE)
        self.assertIn("/var/lib/tor-release/schema-version", STAGE)
        self.assertIn("installed\" != \"$required", STAGE)
        self.assertIn("can_activate=false", STAGE)
        self.assertTrue(activation_has_schema_guard(STAGE))
        self.assertLess(
            STAGE.index("Check root-verified SQLx schema"),
            STAGE.index("Activate verified release only"),
        )

    def test_guard_is_not_just_a_comment(self):
        unsafe = STAGE.replace(
            "if: env.AUTO_INSTALL == 'true' && steps.schema_guard.outputs.can_activate == 'true'",
            "if: env.AUTO_INSTALL == 'true'",
        )
        self.assertFalse(activation_has_schema_guard(unsafe))

    def test_schema_marker_written_only_by_privileged_go_live(self):
        self.assertIn("SELECT max(version) FROM _sqlx_migrations WHERE success=TRUE", GO_LIVE)
        self.assertIn("schema_dir=/var/lib/tor-release", GO_LIVE)
        self.assertIn("install -d -o root -g root -m 0755", GO_LIVE)
        self.assertIn('chown root:root "$schema_tmp"', GO_LIVE)
        self.assertIn('chmod 0644 "$schema_tmp"', GO_LIVE)
        self.assertIn("TOR_SCHEMA_MARKED", GO_LIVE)

    def test_pg_runtime_requires_separate_root_activation_proof(self):
        self.assertIn("/var/lib/tor-release/pg-activated", STAGE)
        self.assertIn('test "$(cat /var/lib/tor-release/pg-activated)" = PG_ACTIVATED', STAGE)
        self.assertIn('grep -Fxq "Environment=TOR_DEMO_MODE=0"', STAGE)
        self.assertIn('grep -Fxq "Environment=TOR_AUTH_ENABLED=1"', STAGE)
        self.assertIn('grep -Fxq "EnvironmentFile=/etc/tor/tor-app.env"', STAGE)
        self.assertIn('"$installed" != "$required" || "$pg_ready" != verified', STAGE)
        self.assertLess(
            STAGE.index('test "$(cat /var/lib/tor-release/pg-activated)"'),
            STAGE.index("Activate verified release only when"),
        )

    def test_pg_active_proof_written_after_https_and_http_acceptance(self):
        self.assertIn('pg_active_marker="$schema_dir/pg-activated"', GO_LIVE)
        self.assertIn("printf '%s\\n' 'PG_ACTIVATED'", GO_LIVE)
        self.assertIn('chown root:root "$pg_active_tmp"', GO_LIVE)
        self.assertIn('chmod 0644 "$pg_active_tmp"', GO_LIVE)
        self.assertLess(
            GO_LIVE.index('https://pandaolate.ru/readyz | grep -qx ok'),
            GO_LIVE.index('pg_active_marker="$schema_dir/pg-activated"'),
        )
        self.assertLess(
            GO_LIVE.index('mv -fT "$pg_active_tmp" "$pg_active_marker"'),
            GO_LIVE.index("TOR_PRODUCTION_OK"),
        )

    def test_current_main_checked_after_staging(self):
        self.assertIn("git ls-remote origin refs/heads/main", STAGE)
        self.assertLess(
            STAGE.index("Transfer verified production binaries"),
            STAGE.index("Refuse obsolete main release before"),
        )


if __name__ == "__main__":
    unittest.main()
