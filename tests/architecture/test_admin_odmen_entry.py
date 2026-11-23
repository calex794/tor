"""Admin alias: retain role enforcement and all existing /admin/ ACLs."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class AdminAliasTests(unittest.TestCase):
    def test_entry_uses_percent_encoded_cyrillic_and_admin_role(self):
        routes = (ROOT / "crates/web/src/auth.rs").read_text()
        module = (ROOT / "crates/web/src/auth/admin_entry.rs").read_text()
        account = (ROOT / "crates/web/src/auth/account_home.rs").read_text()
        self.assertIn('"/%D0%BE%D0%B4%D0%BC%D0%B5%D0%BD", get(admin_entry::page)', routes)
        self.assertIn("state.accounts.session_actor(&digest).await", module)
        self.assertIn("if actor.administrator", module)
        self.assertIn("StatusCode::NOT_FOUND", module)
        self.assertIn("noindex, nofollow", module)
        self.assertIn('href=\\"/одмен\\"', account)
        self.assertNotIn('href=\\"/admin/audience\\"', account)

    def test_does_not_advertise_alias_as_role_security(self):
        doc = (ROOT / "docs/security/admin-odmen-alias.md").read_text()
        self.assertIn("NOT a security boundary", doc)
        self.assertIn("remain accessible", doc)


if __name__ == "__main__":
    unittest.main()
