"""Canonical reading is a bounded read-only projection, not reactivated tracking."""
import hashlib
from ci_workflow_sources import expand_ci_actions
import pathlib
import unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]
def read(p):return (ROOT/p).read_text()
SQL_DIR=ROOT/'crates/reading/src/repositories/audience/canonical'
def assembled_sql(kind):
    return (SQL_DIR/'common.sql').read_bytes()+(SQL_DIR/f'{kind}_tail.sql').read_bytes()

class FlowCanonicalReading(unittest.TestCase):
    def test_existing_sources_only_and_no_new_profile(self):
        sql=assembled_sql('report').decode('utf-8')
        for required in ['reader_return_public_books','reader_chapter_completions','outbox_events','reader_library_items','reader_book_follows','min(c.completed_at)','source_event_id','C-verified-free-position','C-verified-purchased-position','e.payload']:
            self.assertIn(required,sql)
        for forbidden in ['INSERT ','UPDATE ','DELETE ','rec_subjects','rec_feedback','flow_visits','cookie','session_id','email']:
            self.assertNotIn(forbidden,sql)
        dto=read('crates/reading/src/services/audience/canonical.rs')
        self.assertNotIn('sqlx',dto)
        for forbidden in ['user_id:','source_event_id:','purchase_id:','chapter_id:']:
            self.assertNotIn(forbidden,dto)
    def test_reuse09_queries_match_reviewed_original_git_blobs(self):
        # Git blob IDs are a byte-equivalence witness, not a security hash.
        # Future intentional SQL changes must update these reviewed expectations.
        for kind,expected in [
            ('rank','a3527812f7491389196b1be3ec1c9d279ff1a3e1'),
            ('report','29f1f0ee44727dff1a4558ba8599fd0b17ce7e18'),
        ]:
            with self.subTest(kind=kind):
                data=assembled_sql(kind)
                header=f'blob {len(data)}\0'.encode('ascii')
                self.assertEqual(hashlib.sha1(header+data, usedforsecurity=False).hexdigest(),expected)
        repo=(SQL_DIR/'mod.rs').read_text()
        rank=(SQL_DIR/'rank.rs').read_text()
        self.assertIn('concat!(include_str!("common.sql"), include_str!("report_tail.sql"))',repo)
        self.assertIn('concat!(include_str!("common.sql"), include_str!("rank_tail.sql"))',repo)
        self.assertIn('sqlx::query(REPORT_SQL)',repo)
        self.assertIn('sqlx::query(super::RANK_SQL)',rank)

    def test_limits_authority_and_unknown(self):
        repo=read('crates/reading/src/repositories/audience/canonical/mod.rs')
        for required in ['validated()?','begin_report(ReportTimeout::StatementOnly)','CANONICAL_PAGE_SIZE + 1']:
            self.assertIn(required,repo)
        self.assertLess(repo.index('validated()?'),repo.index('begin_report('))
        http=read('crates/web/src/auth/audience_http/canonical.rs')
        self.assertLess(http.index('administrator(&state, &headers)'),http.index('state.store.canonical_report(&query)'))
        for required in ['Duration::from_millis(950)','timeout_at(deadline, state.store.canonical_report(&query))','secure(StatusCode::BAD_REQUEST)']:
            self.assertIn(required,http)
        render=read('crates/web/src/auth/audience_http/render/canonical.rs')
        for required in ['canonical-unavailable','escape_html','comparison_delta','canonical-small','next_after_label']:
            self.assertIn(required,render)
    def test_reuse16a_keeps_each_transaction_profile_and_original_setup(self):
        base='crates/reading/src/repositories/audience'
        helper=read(base+'/transaction.rs')
        isolation='SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY'
        statement="SET LOCAL statement_timeout = '750ms'"
        bounded="SELECT set_config('statement_timeout','750ms',true),set_config('lock_timeout','75ms',true)"
        for sql in (isolation,statement,bounded):
            self.assertEqual(helper.count(sql),1)
        self.assertLess(helper.index(isolation),helper.index('let settings = match timeout'))
        self.assertIn('ReportTimeout::StatementOnly => "'+statement+'"',helper)
        self.assertIn('ReportTimeout::StatementAndLock => {\n                "'+bounded+'"',helper)
        self.assertIn('sqlx::query(settings)',helper)
        for suffix,profile in (
            ('.rs','StatementOnly'),
            ('/intent.rs','StatementOnly'),
            ('/canonical/mod.rs','StatementOnly'),
            ('/canonical/rank.rs','StatementAndLock'),
            ('/closed.rs','StatementAndLock'),
            ('/book_evidence.rs','StatementAndLock'),
            ('/time.rs','StatementAndLock'),
        ):
            with self.subTest(report=suffix):
                source=read(base+suffix)
                self.assertEqual(source.count('begin_report(ReportTimeout::'+profile+')'),1)
                for duplicate in (isolation,statement,bounded,'self.pool.begin()'):
                    self.assertNotIn(duplicate,source)
        command='cargo test -p tor-reading --lib reuse16a_pg_profiles_and_pool_recovery -- --ignored --test-threads=1'
        for file in ['.github/workflows/tor-integration.yml','.github/workflows/tor-pg-reading-analytics.yml']:
            self.assertIn(command,expand_ci_actions(read(file)))

    def test_required_existing_targets_execute_real_new_paths(self):
        for file,needle in [('crates/reading/tests/audience_pg.rs','audience_pg/canonical.rs'),('crates/web/tests/audience_http_pg.rs','canonical::check'),('tools/audience_pg_browser.cjs','await canonical.check')]:
            self.assertIn(needle,read(file))
        for file in ['.github/workflows/tor-integration.yml','.github/workflows/tor-pg-reading-analytics.yml']:
            text=expand_ci_actions(read(file))
            for needle in ['cargo test -p tor-reading --test audience_pg',
                           'cargo test -p tor-web --test audience_http_pg']:
                self.assertIn(needle,text)
        for file in ['.github/workflows/tor-integration.yml','.github/workflows/tor-pg-runtime.yml']:
            text=expand_ci_actions(read(file))
            self.assertNotIn('TOR_REC_REUSE_PLAYWRIGHT=1 bash tools/rec13_browser_acceptance.sh',text)
        for file in ['.github/workflows/tor-integration.yml','.github/workflows/tor-pg-rec-browser.yml']:
            text=expand_ci_actions(read(file))
            self.assertIn('run: bash tools/rec13_browser_acceptance.sh',text)
            self.assertIn('LEGACY_ANALYTICS_DATABASE_URL:',text)
            self.assertIn('COLLECTION_RETIREMENT_APP_DATABASE_URL:',text)

        self.assertIn("node tools/audience_pg_browser.cjs",read("tools/rec13_browser_acceptance.sh"))
