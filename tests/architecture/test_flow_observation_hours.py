"""Observation-hour wiring must not regress into batch-time or individual capture."""
import pathlib
import unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]

def read(path):
    return (ROOT/path).read_text()

class FlowObservationHours(unittest.TestCase):
    def test_clock_is_captured_before_queue_and_carried_into_rollup(self):
        src=read('crates/traffic_behavior/src/services/recorder.rs')
        body=src.split('pub fn try_record_with_flow',1)[1].split('pub fn try_record_sql',1)[0]
        markers=[
            'self.flow_gate.lock()',
            'guard.observation_time()',
            'observed_at.and_then(crate::domain::observation_hour)',
            '.try_send(Message::Event(event, flow, observed_hour))',
            'drop(guard)',
        ]
        positions=[body.index(marker) for marker in markers]
        self.assertEqual(positions,sorted(positions))
        self.assertNotIn('SystemTime::now()',body)
        clock=read('crates/traffic_behavior/src/services/recorder/flow_health.rs')
        self.assertIn('let now = SystemTime::now();',clock)
        self.assertIn('Message::Event(event, flow, observed_hour)',src)
        self.assertIn('record_observed_flow(pageview, observed_hour)',src)
        domain=read('crates/traffic_behavior/src/domain/observed_flow.rs')
        for secret in ['user_id:', 'account_id:', 'session_id:', 'cookie:', 'ip_address:']:
            self.assertNotIn(secret,domain)
        self.assertIn('MAX_OBSERVED_HOURS',domain)

    def test_projection_stays_inside_existing_fenced_transaction(self):
        src=read('crates/traffic_behavior/src/repositories/postgres.rs')
        self.assertIn('observed_flow::stage',src)
        self.assertIn('observed_flow::materialize',src)
        self.assertIn('observed_flow::prune',src)
        schema=read('migrations/202611163827_flow_observation_hours.sql')
        self.assertIn('ril_flow_observed_requires_receipt',schema)
        self.assertIn('BEFORE UPDATE OF status',schema)
        self.assertNotIn('INSERT INTO ril_flow_observed_hourly',schema)

    def test_acceptance_is_wired_into_existing_pg_and_browser_targets(self):
        self.assertIn('mod observed;',read('crates/traffic_behavior/tests/ril_rollups_pg.rs'))
        self.assertIn('mod observed;',read('crates/web/tests/audience_hours_http_pg.rs'))
        browser=read('tools/audience_hours_pg_browser.cjs')
        self.assertIn('audience_observed_pg_helpers.cjs',browser)
        self.assertIn('observed.report(adminPage,90',browser)
        self.assertIn('observed.counts(captureFrom)',browser)
        self.assertIn('audience-observed-table-',browser)

if __name__=='__main__':
    unittest.main()
