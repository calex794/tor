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

    def test_author_general_does_not_bypass_moderator_review(self):
        migration = (ROOT / "migrations/202611010003_rec_general_review_gate.sql").read_text()
        policy = (ROOT / "crates/books/src/repositories/recommendation_policy.rs").read_text()
        self.assertIn("JOIN rec_general_audience_approvals review", migration)
        self.assertIn("review.reviewed_revision=a.revision", migration)
        self.assertIn("review.reviewer_user_id IS NOT NULL", migration)
        self.assertIn('if actor.administrator && audience == "general"', policy)
        self.assertIn("DELETE FROM rec_general_audience_approvals WHERE book_id=$1", policy)
        self.assertIn("DELETE FROM rec_general_audience_approvals WHERE book_id=_book_id", migration)
        self.assertIn("FROM books WHERE id=_book_id FOR UPDATE", migration)
        for table in ("books", "chapters", "chapter_publications", "book_cover_slots"):
            self.assertIn(f"ON {table}", migration)
        self.assertIn("rec_invalidate_review_on_content_change", migration)

    def test_reader_control_does_not_accept_browser_actor(self):
        text=(ROOT/'crates/web/src/recommendation_http/api.rs').read_text()
        for name in ('ImpressionBatch','Activation','FeedbackCommand'):
            body=text.split('struct '+name,1)[1].split('}',1)[0]
            self.assertNotIn('user_id:',body)
            self.assertNotIn('actor:',body)
            self.assertNotIn('trusted:',body)

if __name__=='__main__':
    unittest.main()
