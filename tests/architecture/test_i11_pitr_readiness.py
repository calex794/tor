"""Exercise actual shell readiness guards using local command doubles, not PG proof."""
from pathlib import Path
import os
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = (ROOT / 'tools/i11_pitr_container.sh').read_text()


def function(name):
    match = re.search(r'(?ms)^' + name + r'\(\) \{.*?^\}', SCRIPT)
    if match is None:
        raise AssertionError('missing guard ' + name)
    return match.group(0)


class ReadinessGuards(unittest.TestCase):
    def run_shell(self, definitions, invocation, log=''):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            (work / 'log').mkdir()
            (work / 'log/bad-start.log').write_text(log)
            content = '\n'.join((
                'set -Eeuo pipefail', 'work="$TEST_WORK"',
                'restore_dir="$work/restored"',
                function('wait_for_primary'), function('wait_for_target_failure'),
                'sleep() { :; }', definitions, invocation,
            ))
            return subprocess.run(['bash', '-c', content], text=True,
                                  capture_output=True, timeout=5,
                                  env=dict(os.environ, TEST_WORK=directory))

    def test_accepts_only_sql_ready_promoted_primary(self):
        result = self.run_shell('pg_ctl() { return 0; }\nrestored_sql() { echo f; }',
                                'wait_for_primary 2')
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_retries_connection_rejection_and_standby_before_promotion(self):
        definitions = '''pg_ctl() { return 0; }
restored_sql() {
  local count=0
  [[ ! -f "$work/queries" ]] || read -r count < "$work/queries"
  count=$((count + 1)); echo "$count" > "$work/queries"
  [[ $PGCONNECT_TIMEOUT == 2 && $PGOPTIONS == '-c statement_timeout=2000' ]] || return 9
  case "$count" in
    1) echo 'psql: error: database system is not accepting connections' >&2; return 2 ;;
    2) echo t ;;
    *) echo f ;;
  esac
}'''
        result = self.run_shell(definitions, 'wait_for_primary 2; [[ $(cat "$work/queries") == 3 ]]')
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_dead_postmaster_fails_before_query(self):
        result = self.run_shell('pg_ctl() { return 3; }\nrestored_sql() { echo query-ran; }',
                                'wait_for_primary 2')
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('query-ran', result.stdout)

    def test_readable_standby_never_counts_as_primary(self):
        result = self.run_shell('pg_ctl() { return 0; }\nrestored_sql() { echo t; }',
                                'wait_for_primary 1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('did not become', result.stderr)

    def test_connection_failure_has_bounded_wait(self):
        result = self.run_shell('pg_ctl() { return 0; }\nrestored_sql() { return 2; }',
                                'wait_for_primary 1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('did not become', result.stderr)

    def test_unexpected_sql_state_fails(self):
        result = self.run_shell('pg_ctl() { return 0; }\nrestored_sql() { echo unknown; }',
                                'wait_for_primary 2')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('unexpected', result.stderr)

    def test_negative_case_waits_for_exit_then_requires_target_error(self):
        definitions = '''count=0
pg_ctl() { count=$((count + 1)); ((count < 3)); }'''
        result = self.run_shell(definitions, 'wait_for_target_failure 2; [[ $count == 3 ]]',
                                'FATAL: recovery ended before configured recovery target was reached\n')
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_unrelated_postmaster_crash_does_not_pass_negative_test(self):
        result = self.run_shell('pg_ctl() { return 3; }', 'wait_for_target_failure 2',
                                'FATAL: could not open configuration file\n')
        self.assertNotEqual(result.returncode, 0)

    def test_live_postmaster_cannot_pass_even_with_matching_log(self):
        result = self.run_shell('pg_ctl() { return 0; }', 'wait_for_target_failure 0',
                                'FATAL: recovery ended before configured recovery target was reached\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('stayed running', result.stderr)

    def test_both_guards_are_invoked_before_success_report(self):
        self.assertEqual(SCRIPT.count('wait_for_primary 30'), 1)
        self.assertEqual(SCRIPT.count('wait_for_target_failure 30'), 1)
        self.assertLess(SCRIPT.index('wait_for_primary 30'), SCRIPT.index('string_agg'))
        self.assertLess(SCRIPT.index('wait_for_target_failure 30'), SCRIPT.index("'source_removed': True"))
        self.assertEqual(SCRIPT.count('-c hot_standby=off'), 2)


if __name__ == '__main__':
    unittest.main()
