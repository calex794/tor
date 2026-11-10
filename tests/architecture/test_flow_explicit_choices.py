"""C-owned explicit state stays read-only and never revives optional tracking."""
import pathlib
import unittest
ROOT = pathlib.Path(__file__).resolve().parents[2]
def read(path):
    return (ROOT / path).read_text()

class FlowExplicitChoices(unittest.TestCase):
    def test_only_existing_product_choices_enter_the_read_only_snapshot(self):
        repo = read("crates/reading/src/repositories/audience/intent.rs")
        query = read("crates/reading/src/repositories/audience/intent.sql")
        for required in ["REPEATABLE READ READ ONLY", "750ms", "validate_period(days)?"]:
            self.assertIn(required, repo)
        for required in ["reader_library_items", "reader_book_follows", "count(DISTINCT user_id)", "transaction_timestamp()"]:
            self.assertIn(required, query)
        for forbidden in ["INSERT ", "UPDATE ", "DELETE ", "rec_", "flow_navigation", "outbox_events", "email", "title"]:
            self.assertNotIn(forbidden, query)
        domain = read("crates/reading/src/services/audience/intent.rs")
        for forbidden in ["sqlx", "user_id:", "book_id:", "cookie", "session_id:"]:
            self.assertNotIn(forbidden, domain)

    def test_live_admin_and_whole_request_deadline_are_preserved(self):
        http = read("crates/web/src/auth/audience_http.rs")
        self.assertLess(http.index("administrator(&state, &headers)"), http.index("state.store.intent_report(days)"))
        self.assertIn("timeout_at(deadline, state.store.intent_report(days))", http)
        self.assertIn("&& intents.is_some()", http)
        render = read("crates/web/src/auth/audience_http/render/intent.rs")
        self.assertIn("audience-intents-unavailable", render)
        self.assertIn("super::time(from)", render)
        self.assertIn("&report.previous", render)

    def test_existing_required_targets_execute_the_new_acceptance(self):
        self.assertIn('path = "audience_pg/intent.rs"', read("crates/reading/tests/audience_pg.rs"))
        self.assertIn("intent::choices(&app", read("crates/web/tests/audience_http_pg.rs"))
        browser = read("tools/audience_pg_browser.cjs")
        self.assertIn("audience_intent_pg_helpers.cjs", browser)
        self.assertIn("intents.choices(readerPage,book,csrf,true)", browser)
        self.assertIn("audience-intents-${width}.png", browser)
        for file in [".github/workflows/tor-integration.yml", ".github/workflows/tor-pg-runtime.yml"]:
            ci = read(file)
            for required in ["cargo test -p tor-reading --test audience_pg", "cargo test -p tor-web --test audience_http_pg"]:
                self.assertIn(required, ci)

if __name__ == "__main__":
    unittest.main()
