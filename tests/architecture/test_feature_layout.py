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


if __name__ == "__main__":
    unittest.main()
