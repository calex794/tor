"""REC-13 admin routes cannot silently degrade to ordinary reader authorization."""
import importlib.util
import shutil
import tempfile
import unittest
from pathlib import Path

ROOT=Path(__file__).resolve().parents[2]
SPEC=importlib.util.spec_from_file_location('rec13_guard_checker',ROOT/'tools/check_ril_governance.py')
CHECKER=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)

class RecAdminGuardTests(unittest.TestCase):
    def copy(self):
        temp=tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root=Path(temp.name)
        for folder in ('crates','docs','tools','architecture'):
            shutil.copytree(ROOT/folder,root/folder)
        for file in ('Cargo.toml','AGENTS.md'):
            shutil.copy2(ROOT/file,root/file)
        return root

    def test_role_guard_is_present_and_independent(self):
        text=(ROOT/'crates/web/src/recommendation_http/admin.rs').read_text()
        self.assertIn('session_actor(',text)
        self.assertIn('!verified.administrator',text)
        self.assertIn('StatusCode::FORBIDDEN',text)
        checker=(ROOT/'tools/check_ril_governance.py').read_text()
        self.assertIn('recommendation administration policy downgraded',checker)
        self.assertIn('missing independent administrator role guard',checker)

    def test_rec13_uses_canonical_b_policy_not_parallel_approval(self):
        migration = (ROOT / "migrations/202610310200_rec_feedback_system.sql").read_text()
        policy = (ROOT / "crates/books/src/repositories/recommendation_policy.rs").read_text()
        author = (ROOT / "crates/web/src/recommendation_http/author.rs").read_text()
        self.assertIn("CREATE VIEW rec_public_books", migration)
        self.assertIn("b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted", migration)
        self.assertIn("b.access_level='basic'", migration)
        self.assertIn("COALESCE(a.audience,'general')='general'", migration)
        self.assertNotIn("rec_general_audience_approvals", migration)
        self.assertNotIn("rec_review_revoke_", migration)
        self.assertFalse((ROOT / "migrations/202611010003_rec_general_review_gate.sql").exists())
        self.assertFalse((ROOT / "migrations/202611010004_rec_restore_b_discovery.sql").exists())
        self.assertNotIn("rec_general_audience_approvals", policy)
        self.assertNotIn("settings.reviewed", author)
        self.assertIn("!verified.administrator", (ROOT / "crates/web/src/recommendation_http/admin.rs").read_text())

    def test_reader_control_does_not_accept_browser_actor(self):
        text=(ROOT/'crates/web/src/recommendation_http/api.rs').read_text()
        for name in ('ImpressionBatch','Activation','FeedbackCommand'):
            body=text.split('struct '+name,1)[1].split('}',1)[0]
            self.assertNotIn('user_id:',body)
            self.assertNotIn('actor:',body)
            self.assertNotIn('trusted:',body)

if __name__=='__main__':
    unittest.main()
