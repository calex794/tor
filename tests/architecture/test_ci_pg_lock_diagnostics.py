"""Keep lock diagnostics confined to the disposable analytics database."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"
STEP = "      - name: Enable bounded lock diagnostics only on disposable analytics DB\n"
EXPECTED = {
    "log_lock_waits": "on",
    "deadlock_timeout": "'20ms'",
    "log_error_verbosity": "'verbose'",
    "log_parameter_max_length": "0",
    "log_parameter_max_length_on_error": "0",
}


class PgLockDiagnosticsWiring(unittest.TestCase):
    def test_scoped_and_paired_ci_diagnostics(self):
        steps = []
        for filename in ("tor-integration.yml", "tor-pg-reading-analytics.yml"):
            content = (WORKFLOWS / filename).read_text()
            with self.subTest(filename=filename):
                self.assertEqual(content.count(STEP), 1)
                job = re.split(r"\n  [a-z][a-z0-9-]+:\n",
                               content.split("\n  pg-reading-analytics:\n", 1)[1],
                               maxsplit=1)[0]
                self.assertIn("image: postgres:18", job)
                self.assertIn("PGHOST: 127.0.0.1", job)
                self.assertLess(job.index("Prepare isolated database"), job.index(STEP))
                self.assertLess(job.index("Apply real SQLx migrations twice"), job.index(STEP))
                self.assertLess(job.index(STEP), job.index("Prepare explicit legacy and installed-upgrade analytics databases"))
                self.assertIn("run: bash tools/ci_prepare_isolated_pg.sh", job)
                step = job.split(STEP, 1)[1].split("      - name:", 1)[0]
                steps.append(step)
                self.assertEqual(
                    dict(re.findall(r"(?m)^          ALTER DATABASE tor_ci SET (\w+) = ([^;]+);$", step)),
                    EXPECTED,
                )
                self.assertEqual(step.count("ALTER DATABASE"), len(EXPECTED))
                self.assertNotIn("ALTER SYSTEM", step)
                self.assertNotRegex(step, r"(?i)\bSET\s+(?:lock_timeout|statement_timeout)\b")
                self.assertIn('test "$observed" = t', step)
                self.assertIn('current_user = \'tor_app\'', step)
                self.assertIn('current_database() = \'tor_ci\'', step)
                self.assertIn('"$TEST_DATABASE_URL"', step)
                self.assertIn("CI_PG_LOCK_DIAGNOSTICS_ENABLED", step)
                for key in EXPECTED:
                    self.assertIn("current_setting('" + key + "')", step)
        self.assertEqual(steps[0], steps[1])


if __name__ == "__main__":
    unittest.main()
