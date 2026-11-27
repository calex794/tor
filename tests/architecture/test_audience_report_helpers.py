"""Keep independent historical analytics journeys while sharing exact fixture mechanics."""
from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools"


class AudienceReportHelperTests(unittest.TestCase):
    def test_three_independent_consumers_keep_their_guards(self):
        for report in ("books", "groups", "sources"):
            with self.subTest(report=report):
                path = TOOLS / f"audience_{report}_pg_browser.cjs"
                source = path.read_text(encoding="utf-8")
                shared = "require('./audience_report_pg_helpers.cjs')({db,base})"
                self.assertEqual(source.count(shared), 1)
                self.assertLess(source.index("requires disposable loopback legacy analytics baseline 191"),
                                source.index(shared))
                for name in ("sql", "scalar", "csrf", "privateResponse", "chapter", "read"):
                    self.assertIsNone(re.search(rf"^\s*(?:async )?function {name}\(", source, re.M))
                self.assertIn("async function register(", source)
                subprocess.run(["node", "--check", str(path)], check=True)
        for report in ("book_returns", "group_returns", "returns"):
            source = (TOOLS / f"audience_{report}_pg_browser.cjs").read_text(encoding="utf-8")
            self.assertNotIn("audience_report_pg_helpers.cjs", source)

    def test_three_return_consumers_keep_distinct_modes(self):
        for report, bounded, strict in (("book_returns", "true", "false"),
                                        ("group_returns", "true", "true"),
                                        ("returns", "false", "false")):
            with self.subTest(report=report):
                path = TOOLS / f"audience_{report}_pg_browser.cjs"
                source = path.read_text(encoding="utf-8")
                shared = ("require('./audience_returns_pg_helpers.cjs')({db,base,"
                          f"boundedSql:{bounded},requireReaderUrl:{strict}" + "})")
                self.assertEqual(source.count(shared), 1)
                self.assertLess(source.index("requires disposable loopback legacy analytics baseline 191"),
                                source.index(shared))
                self.assertLess(source.index(shared),
                                source.index("require('./audience_retirement_fixtures.cjs')"))
                for name in ("sql", "scalar", "csrf", "privateResponse", "chapter", "progress", "metric"):
                    self.assertIsNone(re.search(rf"^\s*(?:async )?function {name}\(", source, re.M))
                self.assertIn("async function register(", source)
                subprocess.run(["node", "--check", str(path)], check=True)
        helper = (TOOLS / "audience_returns_pg_helpers.cjs").read_text(encoding="utf-8")
        self.assertIn("if (requireReaderUrl)", helper)
        self.assertIn("progress runs from the actual AJAX reader with its live session", helper)

    def test_returns_helper_behavior_without_external_services(self):
        subprocess.run(["node", "--test", str(TOOLS / "audience_returns_pg_helpers.test.cjs")],
                       cwd=ROOT, check=True)

    def test_shared_helper_behavior_without_external_services(self):
        subprocess.run(["node", "--test", str(TOOLS / "audience_report_pg_helpers.test.cjs")],
                       cwd=ROOT, check=True)


if __name__ == "__main__":
    unittest.main()
