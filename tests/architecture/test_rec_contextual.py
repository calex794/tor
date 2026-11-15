"""Explicit model boundary and negative administrator wiring mutations."""
import importlib.util,json,shutil,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('ctx_governance',ROOT/'tools/check_ril_governance.py')
guard=importlib.util.module_from_spec(spec);spec.loader.exec_module(guard)
class ContextualModelTests(unittest.TestCase):
    def copy(self):
        temp=tempfile.TemporaryDirectory();self.addCleanup(temp.cleanup);root=Path(temp.name)
        for folder in ('crates','docs','tools','architecture'):shutil.copytree(ROOT/folder,root/folder)
        for name in ('Cargo.toml','AGENTS.md'):shutil.copy2(ROOT/name,root/name)
        return root
    def test_new_model_does_not_restart_retired_collection(self):
        builder=(ROOT/'crates/recommendations/src/repositories/feedback/contextual/build.rs').read_text()
        serving=(ROOT/'crates/recommendations/src/repositories/feedback/contextual/serving.rs').read_text()
        worker=(ROOT/'crates/web/src/recommendation_worker.rs').read_text()
        self.assertIn('sources::book_sources(',builder)
        self.assertIn('model_version=$1',builder)
        for old in ('rec_reader_signals','rec_assignments','rec_subjects','refresh_quality','tick_feedback','advance_generation('):
            self.assertNotIn(old,builder+serving+worker)
        self.assertNotIn('::global_sources(',builder)
        self.assertIn('ticket: None',serving)
        self.assertIn('personalized: false',serving)
        self.assertIn('rec_book_preferences',serving)
        self.assertIn('CREATE TRIGGER rec_contextual_policy_guard',(ROOT/'migrations/202611198437_rec_contextual_model.sql').read_text())
    def test_nested_handler_cannot_drop_real_admin_call(self):
        root=self.copy();p=root/'crates/web/src/recommendation_http/admin/contextual.rs'
        text=p.read_text().replace('actor(state, headers).await?;', '')
        p.write_text(text)
        errors=guard.check(root)
        self.assertTrue(any('GET /api/v1/admin/recommendations/contextual' in e and 'administrator' in e for e in errors),errors)
    def test_mutating_delegate_cannot_drop_csrf(self):
        root=self.copy();p=root/'crates/web/src/recommendation_http/admin/contextual.rs'
        p.write_text(p.read_text().replace('csrf(&state, &headers, &token)','always_ok(&state, &headers, &token)'))
        errors=guard.check(root)
        self.assertTrue(any('/recommendations/contextual' in e and 'Origin+CSRF' in e for e in errors),errors)
    def test_model_admin_cannot_become_reader_policy(self):
        root=self.copy();p=root/'docs/contracts/ril-route-guards-v1.json';data=json.loads(p.read_text())
        for row in data['routes']:
            if row['template']=='/me/recommendations/contextual':row['policy']='scoped'
        p.write_text(json.dumps(data))
        self.assertTrue(any('recommendation administration policy downgraded' in e for e in guard.check(root)))
if __name__=='__main__':unittest.main()
