"""Keep causal import evidence wired into real S059 CI; not runtime proof."""
from pathlib import Path
import ast
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


def read(path):
    return (ROOT / path).read_text(encoding='utf-8')


class S059BackgroundTrace(unittest.TestCase):
    def test_origin_is_inserted_with_job_and_cannot_be_rewritten(self):
        migration = read('migrations/001907482365_chapter_import_trace_origin.sql')
        self.assertIn('ADD COLUMN origin_trace_id bytea', migration)
        self.assertIn('octet_length(origin_trace_id) = 16', migration)
        self.assertIn('NEW.origin_trace_id IS DISTINCT FROM OLD.origin_trace_id', migration)
        self.assertIn('BEFORE UPDATE OF origin_trace_id', migration)
        repo = read('crates/imports/src/repositories/postgres.rs')
        self.assertIn('.bind(self.origin_trace_id.map(|id| id.to_be_bytes().to_vec()))', repo)
        self.assertIn('origin_trace_id: decode_origin(&row)?', repo)
        self.assertNotIn('SET origin_trace_id', repo)
        http = read('crates/web/src/chapter_import_queue_http.rs')
        self.assertIn('Option<Extension<crate::observability::TraceId>>', http)
        self.assertIn('.with_origin_trace_id(trace.map(|Extension(id)| id.0))', http)
        payload = http.split('struct QueueInput {', 1)[1].split('}', 1)[0]
        self.assertNotIn('trace', payload)

    def test_attempts_are_separate_and_interruptions_cannot_claim_success(self):
        worker = read('crates/web/src/chapter_import_worker.rs')
        self.assertIn('ImportSpan::new(claim.origin_trace_id, claim.attempts)', worker)
        for phase in range(1, 5):
            self.assertIn(f'span.enter({phase})', worker)
        self.assertIn('span.finish(outcome)', worker)
        span = read('crates/web/src/observability/background.rs')
        self.assertIn('impl Drop for ImportSpan', span)
        self.assertIn('self.finish(Outcome::Interrupted)', span)
        self.assertIn('try_publish_background', span)
        self.assertNotIn('sqlx::', span)
        self.assertNotIn('rusqlite::', span)
        ril = read('crates/web/src/ril.rs')
        self.assertIn('response.status().is_success()', ril)
        self.assertIn('background::retain_root(&trace)', ril)

    def test_existing_required_browser_job_executes_new_real_scenario(self):
        entry = read('tools/operator_trace_pg_smoke.py')
        self.assertIn('run_background_acceptance(output, database, password)', entry)
        self.assertIn('SELECT pg_sleep(1.5)', entry)
        self.assertIn('tools/operator_trace_pg_browser.cjs', entry)
        extension = read('tools/operator_trace_background_acceptance.py')
        self.assertIn("'--test', 's059_background_pg'", extension)
        self.assertIn("'s059_http_import_retry_and_restart_evidence'", extension)
        self.assertIn('check=True', extension)
        self.assertIn('report.get(key) is not False', extension)
        workflow = read('.github/workflows/tor-integration.yml')
        self.assertIn('python3 tools/operator_trace_pg_smoke.py', workflow)
        test = read('crates/web/tests/s059_background_pg.rs')
        for required in ('support::wait_entered()', 'store.kill()', 'lost.kill()',
                         'Outcome::Interrupted', 'Outcome::Stale', 'Outcome::Failed',
                         'root(retry.parent).await', 'operator_trace_background_browser.cjs'):
            self.assertIn(required, test)
        support = read('crates/web/tests/s059_background_pg/support.rs')
        self.assertIn('std::env::current_exe()', support)
        self.assertIn('ChapterImportWorker::new', support)
        self.assertIn('tor-trace-store', support)

    def test_operator_authority_and_bounded_reader_survive(self):
        page = read('crates/web/src/operator_traces.rs')
        self.assertLess(page.index('if !authorized(&headers, secret)'),
                        page.index('background::inspect(socket, trace_id)'))
        self.assertIn('return denied()', page)
        self.assertIn('mod tests;', page)
        self.assertIn('Some(&secret)', read('crates/web/src/operator_traces/tests.rs'))
        child = read('crates/web/src/operator_traces/background.rs')
        self.assertIn('Semaphore::new(4)', child)
        self.assertLess(child.index('spawn_blocking(move ||'), child.index('let _permit = permit'))
        self.assertIn('StatusCode::SERVICE_UNAVAILABLE', child)
        self.assertIn('не доказывает', child)

    def test_acceptance_helpers_are_syntax_valid(self):
        for name in ('operator_trace_pg_smoke.py', 'operator_trace_background_acceptance.py'):
            ast.parse(read('tools/' + name), filename=name)
        subprocess.run(['node', '--check', str(ROOT / 'tools/operator_trace_background_browser.cjs')],
                       check=True, capture_output=True, timeout=10)


if __name__ == '__main__':
    unittest.main()
