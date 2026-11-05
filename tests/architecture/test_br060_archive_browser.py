"""BR-060: require the real PostgreSQL/Chromium archive regression in CI."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class Br060ArchiveBrowserTests(unittest.TestCase):
    def test_real_browser_journey_is_a_required_reader_browser_step(self):
        workflow = (ROOT / ".github/workflows/tor-integration.yml").read_text()
        script = "tools/br060_archive_browser_pg.cjs"
        self.assertIn("node --check " + script, workflow)
        self.assertIn(
            'TOR_READER_SHOTS="$RUNNER_TEMP/reader-shots" node ' + script,
            workflow,
        )
        self.assertEqual(workflow.count("node " + script + "\n"), 1)

    def test_disposable_sql_and_real_http_cannot_be_replaced_by_mocks(self):
        code = (ROOT / "tools/br060_archive_browser_pg.cjs").read_text()
        for requirement in (
            "process.env.GITHUB_ACTIONS !== 'true'",
            "dbUrl.pathname !== '/tor_ci'",
            "dbUrl.username !== 'tor_app'",
            "execFileSync('psql'",
            "approvePendingBookForIsolatedCi(book)",
            "page.waitForURL(base + '/me')",
            "owner.locator('[name=action]').inputValue(), 'start'",
            "moderation/books/deletions",
            "button[name=decision][value=approve]",
            "request_revision",
            "INSERT INTO account_roles",
            "book_ordinary_deletion_decisions",
            "owner request must not deindex before independent site approval",
            "readers must retain previously approved public access until moderation",
            "independent moderation approval refused",
            "reader.waitForResponse(response =>",
            "response.request().method() === 'PUT'",
            "freeProgressRestored === '1'",
            "reader_chapter_completions",
            "await register(outsider,",
        ):
            with self.subTest(requirement=requirement):
                self.assertIn(requirement, code)
        self.assertNotIn(".route(", code, "network mocks are not an acceptance test")
        self.assertNotIn("route.fulfill(", code)
        self.assertNotIn("TEST_DATABASE_URL ||", code)
        self.assertNotIn("['complete', 'Окончательно удалить книгу']", code)
        self.assertNotIn("INSERT INTO book_ordinary_deletion_decisions", code)
        self.assertNotIn("UPDATE books SET lifecycle_status='ordinary_deleted'", code)


if __name__ == "__main__":
    unittest.main()
