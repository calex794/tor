"""S020 P0: protect accepted gated age editor against stale-branch merges.

A cross-workstream merge must never silently drop a merged, disabled-by-default
security surface and its disposable PG/HTTP acceptance job.
"""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
ROUTE = "/me/books/{id}/settings/access-level"
PG_TEST = "cargo test -p tor-web --test s020_age_editor_http_pg -- --ignored --test-threads=1"
BROWSER_PG_TEST = "cargo test -p tor-web --test s020_age_editor_browser_pg -- --ignored --test-threads=1"


class S020GatedEditorSurvival(unittest.TestCase):
    def read(self, path: str) -> str:
        return (ROOT / path).read_text(encoding="utf-8")

    def test_guarded_routes_and_runtime_flag_survive_unrelated_merges(self):
        handler = self.read("crates/web/src/auth/book_access_level.rs")
        source = self.read("crates/web/src/auth.rs")
        state = self.read("crates/web/src/auth/types.rs")
        settings = self.read("crates/web/src/auth/book_origin_metadata.rs")
        contract = self.read("docs/contracts/ril-route-guards-v1.json")

        self.assertIn(ROUTE, handler)
        self.assertIn("get(edit).post(save)", handler)
        self.assertEqual(handler.count("if !state.age_editor_enabled"), 2)
        self.assertIn("same_origin(&headers)", handler)
        self.assertIn("valid_csrf(&headers", handler)
        self.assertIn("verified_upgrade_book_access_level(", handler)
        self.assertIn("manageable_access_level(", handler)
        self.assertIn("mod book_access_level;", source)
        self.assertIn('std::env::var("TOR_S020_ACCESS_EDITOR_ENABLED").as_deref() == Ok("1")', source)
        self.assertIn("age_editor_enabled: bool", state)
        self.assertIn("super::book_access_level::router()", settings)
        self.assertIn(ROUTE, settings)
        self.assertEqual(contract.count(f'"template": "{ROUTE}"'), 2)

    def test_pg_acceptance_and_registry_remain_mandatory(self):
        self.assertIn(
            "s020_age_editor_is_off_by_default_and_monotonic_under_live_http",
            self.read("crates/web/tests/s020_age_editor_http_pg.rs"),
        )
        for path in (".github/workflows/tor-integration.yml",
                     ".github/workflows/tor-pg-runtime.yml"):
            self.assertEqual(self.read(path).count(PG_TEST), 1, path)
        self.assertIn(
            "s020_owner_age_editor_real_chromium_mobile_and_desktop",
            self.read("crates/web/tests/s020_age_editor_browser_pg.rs"),
        )
        browser = self.read("tools/s020_age_editor_chromium.cjs")
        self.assertIn("chromium.launch", browser)
        self.assertIn("S020 owner age editor Chromium PASS: desktop, mobile, grant, revoke", browser)
        for path in (".github/workflows/tor-integration.yml",
                     ".github/workflows/tor-pg-runtime.yml"):
            self.assertEqual(self.read(path).count(BROWSER_PG_TEST), 1, path)
        registry = self.read("crates/route_intelligence/src/domain/routes.rs")
        self.assertIn(f'RouteRegistration::new("{ROUTE}", "GET"', registry)
        self.assertIn(f'RouteRegistration::new("{ROUTE}", "POST"', registry)


if __name__ == "__main__":
    unittest.main()
