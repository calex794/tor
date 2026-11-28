"""REUSE-16B: keep report-only PostgreSQL setup in the F-owned adapter."""
from pathlib import Path
import unittest

from ci_workflow_sources import expand_ci_actions

ROOT = Path(__file__).resolve().parents[2]
REPO = ROOT / "crates/recommendations/src/repositories"


class ReportTransactionTests(unittest.TestCase):
    def test_exact_statement_only_profile_and_validated_call_sites(self):
        helper = (REPO / "report_transaction.rs").read_text().split("#[cfg(test)]", 1)[0]
        isolation = "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY"
        timeout = "SET LOCAL statement_timeout = '750ms'"
        self.assertIn("mod report_transaction;", (REPO / "mod.rs").read_text())
        self.assertEqual(helper.count(isolation), 1)
        self.assertEqual(helper.count(timeout), 1)
        self.assertLess(helper.index("pool.begin()"), helper.index(isolation))
        self.assertLess(helper.index(isolation), helper.index(timeout))
        self.assertNotIn('sqlx::query("SELECT set_config', helper)
        self.assertNotIn("SET LOCAL lock_timeout", helper)
        for path, validation in (
            ("audience.rs", "validate_period(days)?"),
            ("audience/books.rs", "BookDiscoveryQuery::with_missing("),
            ("audience/groups.rs", "GroupDiscoveryQuery::new("),
            ("flow_funnel/report.rs", "if !FLOW_VISIT_DAYS.contains(&days)"),
            ("visits/report.rs", "if !FLOW_VISIT_DAYS.contains(&days)"),
        ):
            with self.subTest(path=path):
                source = (REPO / path).read_text()
                call = "begin_read_report(&self.pool).await.map_err(unavailable)?"
                self.assertEqual(source.count(call), 1)
                self.assertLess(source.index(validation), source.index(call))
                self.assertLess(source.index(call), source.index("tx.commit().await"))
                for duplicate in ("self.pool.begin()", isolation, "SET LOCAL statement_timeout"):
                    self.assertNotIn(duplicate, source)

    def test_helper_is_not_used_by_writers_or_other_repository_paths(self):
        callers = {
            "audience.rs", "audience/books.rs", "audience/groups.rs",
            "flow_funnel/report.rs", "visits/report.rs",
        }
        actual = {
            path.relative_to(REPO).as_posix()
            for path in REPO.rglob("*.rs")
            if "begin_read_report(" in path.read_text()
        }
        self.assertEqual(actual, callers | {"report_transaction.rs"})

    def test_real_postgresql_acceptance_remains_required(self):
        command = (
            "cargo test -p tor-recommendations --lib "
            "reuse16b_pg_statement_only_and_pool_recovery -- --ignored --test-threads=1"
        )
        for workflow in ("tor-integration.yml", "tor-pg-reading-analytics.yml"):
            with self.subTest(workflow=workflow):
                source = (ROOT / ".github/workflows" / workflow).read_text()
                self.assertIn(command, expand_ci_actions(source))


if __name__ == "__main__":
    unittest.main()
