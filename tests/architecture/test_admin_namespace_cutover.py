"""Admin namespace migration inventory and final regression guard.

The failing assertion lists paths only: no source lines, tokens or config.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
OLD_PUBLIC_PREFIX = re.compile(
    r"(?:"
    r"(?<![\w./-])/admin"
    r"|/api/v1/admin"
    r"|/read/admin"
    r"|(?<![\w./-])/assets/newboo-admin-(?:reader\.mjs|support\.js)"
    r"|/admin-(?:history|unsealed|sealed-history)"
    r"|/me/recommendations/(?:manage|contextual|shelf-preview)"
    r"|https?://[^/\s\x22\x27\x60]+/admin"
    r")(?=/|-|[\x22\x27\x60\s?#)$|(]|$)", re.I
)
TEXT = {".rs", ".py", ".cjs", ".js", ".mjs", ".html", ".json", ".css",
        ".sh", ".ts", ".tsx", ".md", ".sql", ".toml", ".tsv"}
RUNTIME = ("crates/web/src", "crates/web/assets", "crates/web/templates",
           "crates/route_intelligence/src")
OTHER = ("tools", "crates/web/tests", "tests/architecture",
         "docs/contracts", ".github/workflows")


def legacy_public_matches(content):
    # JavaScript regex literals escape slashes; they are still URL paths.
    # Remove the regex delimiter immediately before an escaped leading slash.
    normalized = content.replace(r"/\/", "/").replace(r"\/", "/")
    return OLD_PUBLIC_PREFIX.findall(normalized)


def inventory(paths):
    found = []
    for top in paths:
        base = ROOT / top
        if not base.exists():
            continue
        for path in sorted(base.rglob("*")):
            if path.is_symlink() or not path.is_file() or path.suffix not in TEXT:
                continue
            if path.name == "test_admin_namespace_cutover.py":
                continue
            try:
                content = path.read_text(encoding="utf-8")
            except (UnicodeError, OSError):
                continue
            count = len(legacy_public_matches(content))
            if count:
                found.append((path.relative_to(ROOT).as_posix(), count))
    return found


class AdminNamespaceCutoverTests(unittest.TestCase):
    def test_no_live_admin_routes_or_links_with_old_prefix(self):
        old = inventory(RUNTIME)
        other = [(path, count) for path, count in inventory(OTHER)
                 if not path.endswith(".md")
                 and path not in (
                     "tools/a09_honeypot_browser.cjs",
                     "tools/audience_pg_browser.cjs",  # exact negative HTTP test
                 )]
        summary = [f"ADMIN_NAMESPACE_{scope} {path} {n}"
                   for scope, items in (("LIVE", old), ("OTHER", other))
                   for path, n in items]
        self.assertEqual(old + other, [], "\n" + "\n".join(summary[:280]))

    def test_inventory_pattern_distinguishes_routes_from_code_paths(self):
        for address in (
            '"/admin/mail"', 'href="/admin/audience"',
            '"/admin-history"', '/api/v1/admin/jobs/1/retry',
            '/read/admin/12', 'https://pandaolate.ru/admin/support',
            '"/me/recommendations/manage"',
            '"/me/recommendations/contextual"',
            '"/me/recommendations/shelf-preview"',
            r"/\/admin\/support\/\d+$/",
            r"/^\/admin$/",
            r"/\/admin(?:\/|$)/",
            r"/\/api\/v1\/admin\/jobs/",
            r"/\/read\/admin\/12/",
            r"/\/api\/v1\/reader\/chapters\/123\/admin-history$/",
            'import("/assets/newboo-admin-reader.mjs")',
            'src="/assets/newboo-admin-support.js"',
            '"/api/v1/reader/chapters/123/admin-history"',
        ):
            self.assertTrue(legacy_public_matches(address), address)
        for not_route in (
            "crates/web/src/recommendation_http/admin/contextual.rs",
            "/__bp_honeypot/admin/private", "/go/admin",
            "internal/admin route", "moderator/admin",
            'include_str!("../../assets/newboo-admin-reader.mjs")',
            'data-admin-unsealed',
            r"/\/odmen\/support\/\d+$/",
        ):
            self.assertFalse(legacy_public_matches(not_route), not_route)

    def test_negative_http_fixture_lists_only_four_explicit_legacy_routes(self):
        browser = (ROOT / "tools/audience_pg_browser.cjs").read_text(
            encoding="utf-8")
        legacy = legacy_public_matches(browser)
        self.assertEqual(len(legacy), 4)
        for expected in (
            "/admin/audience",
            "/admin/finance/refunds",
            "/api/v1/admin/recommendations/quality",
            "/admin/finance/refunds/prepare",
        ):
            self.assertEqual(browser.count("'" + expected + "'"), 1)
        self.assertIn("obsoleteWrite.status(), 404", browser)
        self.assertIn("denied.status(), 404", browser)

    def test_administrator_only_recommendation_pages_are_not_public_me_routes(self):
        import json
        data = json.loads((ROOT / "docs/contracts/ril-route-guards-v1.json")
                          .read_text(encoding="utf-8"))
        expected = {
            ("GET", "/odmen/recommendations/manage"),
            ("POST", "/odmen/recommendations/manage"),
            ("GET", "/odmen/recommendations/contextual"),
            ("POST", "/odmen/recommendations/contextual"),
            ("GET", "/odmen/recommendations/shelf-preview"),
        }
        actual = {(item["method"], item["template"]) for item in data["routes"]
                  if item["policy"] == "administrator"
                  and item["template"].startswith("/odmen/recommendations/")}
        self.assertEqual(actual, expected)
        self.assertFalse(any(item["policy"] == "administrator"
                             and item["template"].startswith("/me/recommendations/")
                             for item in data["routes"]))

    def test_unrelated_honeypot_name_is_intentionally_preserved(self):
        decoy = (ROOT / "tools/a09_honeypot_browser.cjs").read_text(encoding="utf-8")
        self.assertIn("__bp_honeypot/admin/private", decoy)


if __name__ == "__main__":
    unittest.main()
