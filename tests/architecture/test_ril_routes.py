"""Negative tests: a new unclassified route cannot silently enter TOR."""
from __future__ import annotations

from pathlib import Path
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_ril_routes import check  # noqa: E402


class RilCoverageTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        for src in (ROOT / "crates").glob("*/src/**/*.rs"):
            dst = self.root / src.relative_to(ROOT)
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)

    def mutate(self, file: str, old: str, new: str):
        target = self.root / file
        text = target.read_text(encoding="utf-8")
        self.assertIn(old, text)
        target.write_text(text.replace(old, new, 1), encoding="utf-8")

    def test_current_registered_routes_are_covered(self):
        self.assertEqual([], check(self.root))

    def test_new_route_requires_taxonomy_and_classifier(self):
        self.mutate(
            "crates/web/src/lib.rs",
            '.route("/catalog", get(catalog_http::catalog))',
            '.route("/catalog", get(catalog_http::catalog))'
            '.route("/new/{id}", get(catalog))',
        )
        self.assertTrue(any("missing RIL taxonomy" in error and "/new/{id}" in error
                            for error in check(self.root)))

    def test_missing_classifier_method_is_detected(self):
        self.mutate(
            "crates/route_intelligence/src/domain/mod.rs",
            '(Some("/login"), "POST") => Some(8)',
            '(Some("/login"), "PATCH") => Some(8)',
        )
        self.assertTrue(any("RIL classifier does not cover taxonomy" in error
                            for error in check(self.root)))

    def test_dynamic_router_registration_fails_closed(self):
        self.mutate(
            "crates/web/src/lib.rs",
            '.route("/catalog", get(catalog_http::catalog))',
            '.route(dynamic_path, get(catalog))',
        )
        self.assertTrue(any("dynamic or non-literal" in error
                            for error in check(self.root)))

    def test_stale_registry_is_detected(self):
        self.mutate(
            "crates/route_intelligence/src/domain/mod.rs",
            'RouteRegistration::new("/", "GET", "/", Exact',
            'RouteRegistration::new("/phantom", "GET", "/", Exact',
        )
        self.assertTrue(any("stale RIL taxonomy" in error
                            for error in check(self.root)))


if __name__ == "__main__":
    unittest.main()
