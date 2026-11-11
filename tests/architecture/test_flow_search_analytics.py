"""Keep external search aggregates separate from reader identities and money."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]
def read(path):
    return (ROOT/path).read_text()
class SearchAnalyticsBoundaries(unittest.TestCase):
    def test_fixed_providers_and_bounded_credentials(self):
        client=read('crates/search/src/repositories/seo_provider/mod.rs')
        for token in ['https://www.googleapis.com','https://api.webmaster.yandex.net','Policy::none()', 'set_sensitive(true)', 'BODY_LIMIT', 'Duration::from_secs(8)']:
            self.assertIn(token,client)
        worker=read('crates/web/src/seo_analytics_worker.rs')
        for token in ['TOR_FLOW_SEO_IMPORT','TOKEN_FILE','spawn_blocking','0o077','file.take(4097)','request_backfill','sync_one']:
            self.assertIn(token,worker)
        self.assertNotIn('TOR_AUTH_ENABLED',worker)
    def test_ingestion_does_not_store_personal_or_financial_rows(self):
        schema=read('migrations/202611237619_flow_search_analytics.sql')
        repo=read('crates/search/src/repositories/seo_analytics/mod.rs')
        for token in ['user_id','session_id','account_id','cookie','wallet','outbox_events','referrer','query_text']:
            self.assertNotIn(token,schema)
        for token in ['FOR UPDATE OF j SKIP LOCKED','epoch','lease_until>clock_timestamp()',"ON CONFLICT(source_id,day)",'request_backfill']:
            self.assertIn(token,repo)
        self.assertIn('READ ONLY',repo)
        self.assertIn('limits(&mut tx, true)',read('crates/search/src/repositories/seo_analytics/report.rs'))
    def test_admin_path_remains_private_and_does_not_contain_sql(self):
        http=read('crates/web/src/auth/audience_http/seo.rs')
        for token in ['administrator','csrf','origin','timeout_at','secure']:
            self.assertIn(token,http)
        self.assertNotIn('sqlx',http)
        self.assertNotIn('SELECT ',http)
        routes=read('crates/web/src/auth/audience_http.rs')
        self.assertIn('/admin/audience/seo',routes)
        self.assertIn('/admin/audience/seo/refresh',routes)
    def test_real_acceptance_targets_and_worker_shutdown_stay_wired(self):
        self.assertIn('mod analytics;',read('crates/search/tests/seo_pg.rs'))
        self.assertIn('mod seo;',read('crates/web/tests/audience_http_pg.rs'))
        self.assertIn('seo::',read('crates/web/tests/audience_http_pg.rs'))
        self.assertIn('audience_seo_pg_helpers.cjs',read('tools/audience_pg_browser.cjs'))
        main=read('crates/web/src/main.rs')
        self.assertIn('seo_analytics_worker::start',main)
        self.assertIn('seo_analytics',main)
if __name__=='__main__':
    unittest.main()
