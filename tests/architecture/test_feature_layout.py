"""Mutation tests for the TOR feature architecture gate (no Rust compiler required)."""
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "tools"))
from check_feature_layout import check


class FeatureLayoutTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)

    def put(self, name, text=""):
        f = self.root / name
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_text(text, encoding="utf8")

    def scaffold(self):
        self.put("Cargo.toml", '[workspace]\nmembers = ["crates/contracts", "crates/books", "crates/web"]\nresolver = "2"\n')
        self.put("architecture/modules.toml", '[features]\nbooks = "B"\nreading = "C"\n')
        for name in ("contracts", "books", "web"):
            self.put(f"crates/{name}/Cargo.toml", f'[package]\nname = "tor-{name}"\nversion = "0.1.0"\nedition = "2021"\n')
            self.put(f"crates/{name}/src/lib.rs", "")
        self.put("crates/books/src/services/mod.rs", "pub fn publish() {}\n")
        self.put("crates/books/tests/service.rs", "#[test] fn works() {}\n")

    def test_empty_repo_is_acceptable_before_bootstrap(self):
        self.assertEqual(check(self.root), [])

    def test_valid_feature_package(self):
        self.scaffold()
        self.assertEqual(check(self.root), [])

    def test_paid_reader_repository_reference_is_forbidden(self):
        self.scaffold()
        path = "crates/web/src/pages/reader_paid.rs"
        self.put(path, "use tor_commerce::services::reader_views::ReadablePurchase;\n")
        self.assertEqual(check(self.root), [])
        self.put(path, "use tor_commerce::repositories::entitlements::ReadablePurchase;\n")
        self.assertTrue(any("web page repository coupling increased" in error for error in check(self.root)))

    def test_library_repository_budget_is_reduced_to_eight(self):
        self.scaffold()
        path = "crates/web/src/pages/library.rs"
        line = "use tor_reading::repositories::follow_updates::FollowedReleasePage;\n"
        self.put(path, line * 8)
        self.assertEqual(check(self.root), [])
        self.put(path, line * 9)
        self.assertTrue(any("web page repository coupling increased" in error for error in check(self.root)))

    def test_flat_services_forbidden(self):
        self.put("src/services/x.rs", "")
        self.assertTrue(any("root src" in item for item in check(self.root)))

    def test_cross_feature_dependency_forbidden_even_under_alias(self):
        self.scaffold()
        self.put("crates/reading/Cargo.toml", '[package]\nname = "tor-reading"\nversion = "0.1.0"\n')
        self.put("crates/reading/src/lib.rs", "")
        self.put("crates/reading/src/domain/mod.rs", "")
        self.put("crates/reading/tests/works.rs", "#[test] fn works() {}\n")
        self.put("Cargo.toml", '[workspace]\nmembers = ["crates/contracts", "crates/books", "crates/web", "crates/reading"]\n')
        with (self.root / "crates/books/Cargo.toml").open("a", encoding="utf8") as f:
            f.write('\n[dependencies]\nalias = { package = "tor-reading", path = "../reading" }\n')
        self.assertTrue(any("cross-feature dependency" in item for item in check(self.root)))

    def test_web_sqlx_is_allowed_only_for_tests(self):
        self.scaffold()
        manifest = '[package]\\nname = "tor-web"\\nversion = "0.1.0"\\nedition = "2021"\\n'
        self.put("crates/web/Cargo.toml", manifest + '[dev-dependencies]\\nsqlx = "0.8"\\n')
        self.assertEqual(check(self.root), [])
        self.put(
            "crates/web/Cargo.toml",
            manifest + '[dependencies]\\nsqlx = "0.8"\\n[dev-dependencies]\\nsqlx = "0.8"\\n',
        )
        self.assertTrue(any("crates/web must not depend on sqlx" in error for error in check(self.root)))
        self.put(
            "crates/web/Cargo.toml",
            manifest + '[dependencies]\\naliased = { package = "sqlx", version = "0.8" }\\n',
        )
        self.assertTrue(any("crates/web must not depend on sqlx" in error for error in check(self.root)))
        self.put(
            "crates/web/Cargo.toml",
            manifest + '[build-dependencies]\\nsqlx = "0.8"\\n',
        )
        self.assertTrue(any("crates/web must not depend on sqlx" in error for error in check(self.root)))

    def test_sqlx_not_allowed_in_service(self):
        self.scaffold()
        self.put("crates/books/src/services/sql.rs", 'fn db() { let _ = sqlx::query("select 1"); }\n')
        self.assertTrue(any("SQLx outside repositories" in item for item in check(self.root)))

    def test_axum_not_allowed_in_domain(self):
        self.scaffold()
        self.put("crates/books/src/domain/mod.rs", "use axum::Router;\n")
        self.assertTrue(any("Axum inside domain" in item for item in check(self.root)))

    def test_unregistered_feature_forbidden(self):
        self.scaffold()
        self.put("crates/utils/Cargo.toml", '[package]\nname = "tor-utils"\nversion = "0.1.0"\n')
        self.put("crates/utils/src/lib.rs", "")
        self.assertTrue(any("unregistered feature" in item for item in check(self.root)))

    def test_missing_feature_tests_forbidden(self):
        self.scaffold()
        (self.root / "crates/books/tests/service.rs").unlink()
        self.assertTrue(any("no colocated tests" in item for item in check(self.root)))

    def test_new_web_page_repository_dependency_is_rejected(self):
        self.scaffold()
        self.put("crates/web/src/pages/new_page.rs", "use tor_books::repositories::PgCatalog;\n")
        self.assertTrue(any("web page repository coupling increased" in error for error in check(self.root)))

    def test_grouped_web_page_repository_dependency_is_rejected(self):
        self.scaffold()
        self.put("crates/web/src/pages/new_page.rs", "use tor_books::{domain::BookKey, repositories::PgCatalog};\n")
        self.assertTrue(any("web page repository coupling increased" in error for error in check(self.root)))

    def test_cleaned_catalog_page_rejects_any_new_repository_dependency(self):
        self.scaffold()
        path = "crates/web/src/pages/public_catalog.rs"
        self.put(path, "use tor_search::services::catalog_taxonomy::CatalogTaxonomyView;\n")
        self.assertEqual(check(self.root), [])
        self.put(path, "use tor_search::repositories::catalog::TaxonomyOptions;\n")
        self.assertTrue(any("web page repository coupling increased" in error for error in check(self.root)))

    def test_cleaned_catalog_taxonomy_selects_rejects_repository_dependency(self):
        self.scaffold()
        path = "crates/web/src/pages/public_catalog/catalog_taxonomy_selects.rs"
        self.put(path, "use tor_search::services::catalog_taxonomy::CatalogTaxonomyView;\n")
        self.assertEqual(check(self.root), [])
        self.put(path, "use tor_search::repositories::catalog::TaxonomyOptions;\n")
        self.assertTrue(any("web page repository coupling increased" in error for error in check(self.root)))

    def test_author_schedule_page_uses_schema_not_repository(self):
        self.scaffold()
        path = "crates/web/src/author_chapter_pages.rs"
        self.put(path, "use tor_books::schemas::ScheduledChapter;\n")
        self.assertEqual(check(self.root), [])
        self.put(path, "use tor_books::repositories::postgres::ScheduledChapter;\n")
        self.assertTrue(any("web page repository coupling increased" in error for error in check(self.root)))

    def test_notification_settings_page_requires_service_type(self):
        self.scaffold()
        path = "crates/web/src/pages/notification_settings.rs"
        self.put(path, "use tor_reading::services::notification_preferences::DeliveryMode;\n")
        self.assertEqual(check(self.root), [])
        self.put(path, "use tor_reading::repositories::DeliveryMode;\n")
        self.assertTrue(any("web page repository coupling increased" in error for error in check(self.root)))

    def test_profile_settings_page_uses_service_view(self):
        self.scaffold()
        path = "crates/web/src/pages/profile_settings.rs"
        self.put(path, "use tor_accounts::services::profile::Profile;\n")
        self.assertEqual(check(self.root), [])
        self.put(path, "use tor_accounts::repositories::profile::Profile;\n")
        self.assertTrue(any("web page repository coupling increased" in error for error in check(self.root)))

    def test_web_router_may_wire_repository(self):
        self.scaffold()
        self.put("crates/web/src/router.rs", "use tor_books::repositories::PgCatalog;\n")
        self.assertEqual(check(self.root), [])

    def test_retention_does_not_start_while_building_router(self):
        self.scaffold()
        self.put("crates/web/src/auth.rs", "fn router_inner() { security_retention::start(accounts); }\n")
        self.assertTrue(any("account retention must not start" in error for error in check(self.root)))


if __name__ == "__main__":
    unittest.main()
