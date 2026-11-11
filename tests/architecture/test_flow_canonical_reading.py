"""Canonical reading is a bounded read-only projection, not reactivated tracking."""
import pathlib
import unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]
def read(p):return (ROOT/p).read_text()
class FlowCanonicalReading(unittest.TestCase):
    def test_existing_sources_only_and_no_new_profile(self):
        sql=read('crates/reading/src/repositories/audience/canonical/report.sql')
        for required in ['reader_return_public_books','reader_chapter_completions','outbox_events','reader_library_items','reader_book_follows','min(c.completed_at)','source_event_id','C-verified-free-position','C-verified-purchased-position','e.payload']:
            self.assertIn(required,sql)
        for forbidden in ['INSERT ','UPDATE ','DELETE ','rec_subjects','rec_feedback','flow_visits','cookie','session_id','email']:
            self.assertNotIn(forbidden,sql)
        dto=read('crates/reading/src/services/audience/canonical.rs')
        self.assertNotIn('sqlx',dto)
        for forbidden in ['user_id:','source_event_id:','purchase_id:','chapter_id:']:
            self.assertNotIn(forbidden,dto)
    def test_limits_authority_and_unknown(self):
        repo=read('crates/reading/src/repositories/audience/canonical/mod.rs')
        for required in ['validated()?','REPEATABLE READ READ ONLY','750ms','CANONICAL_PAGE_SIZE + 1']:
            self.assertIn(required,repo)
        http=read('crates/web/src/auth/audience_http/canonical.rs')
        self.assertLess(http.index('administrator(&state, &headers)'),http.index('state.store.canonical_report(&query)'))
        for required in ['Duration::from_millis(950)','timeout_at(deadline, state.store.canonical_report(&query))','secure(StatusCode::BAD_REQUEST)']:
            self.assertIn(required,http)
        render=read('crates/web/src/auth/audience_http/render/canonical.rs')
        for required in ['canonical-unavailable','escape_html','comparison_delta','canonical-small','next_after_label']:
            self.assertIn(required,render)
    def test_required_existing_targets_execute_real_new_paths(self):
        for file,needle in [('crates/reading/tests/audience_pg.rs','audience_pg/canonical.rs'),('crates/web/tests/audience_http_pg.rs','canonical::check'),('tools/audience_pg_browser.cjs','await canonical.check')]:
            self.assertIn(needle,read(file))
        for file in ['.github/workflows/tor-integration.yml','.github/workflows/tor-pg-runtime.yml']:
            text=read(file)
            for needle in ['cargo test -p tor-reading --test audience_pg','cargo test -p tor-web --test audience_http_pg','bash tools/rec13_browser_acceptance.sh']:
                self.assertIn(needle,text)

        self.assertIn("node tools/audience_pg_browser.cjs",read("tools/rec13_browser_acceptance.sh"))
