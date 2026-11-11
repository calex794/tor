"""Prevent unrelated restore failures from passing the wrong-key test."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
QUERY = "[[ $(restored_sql -c 'SELECT pg_is_in_recovery()') == f ]]"


class RecoveryKeyControl(unittest.TestCase):
    def assert_control_order(self, script):
        # Anchor actual top-level invocations, not SQL inside an earlier helper
        # definition. Definitions do not execute the query when sourced.
        def position(line):
            marker = '\n' + line + '\n'
            self.assertEqual(script.count(marker), 1, 'missing/duplicate command: ' + line)
            return script.index(marker)

        negative = script.index('\nif restore_target "$work/wrong.conf"')
        no_cluster = position('[[ ! -e "$restore_dir/global/pg_control" ]]')
        positive = position('restore_target "$conf" >"$work/log/restore.log" 2>&1')
        ready = position('wait_for_primary 30')
        query = position(QUERY)
        success = script.index("'wrong_key_rejected': True")
        for before, after in zip((negative, no_cluster, positive, ready, query),
                                 (no_cluster, positive, ready, query, success)):
            self.assertLess(before, after)
        command = script.split('restore_target() {', 1)[1].split('\n}', 1)[0]
        self.assertIn('--config="$1"', command)
        self.assertIn('--pg1-path="$restore_dir"', command)
        self.assertIn('--set="$label" --type=name --target=tor_i11_target', command)
        self.assertNotIn('|| true', command)
        self.assertIn('repo1-cipher-pass=deliberately-wrong-test-key/', script)

    def test_negative_and_positive_use_identical_restore_command(self):
        self.assert_control_order((ROOT / 'tools/i11_pitr_container.sh').read_text())

    def test_query_before_positive_restore_is_rejected(self):
        script = (ROOT / 'tools/i11_pitr_container.sh').read_text()
        script = script.replace('\n' + QUERY + '\n', '\n')
        invocation = '\nrestore_target "$conf" >"$work/log/restore.log" 2>&1\n'
        script = script.replace(invocation, '\n' + QUERY + invocation)
        with self.assertRaises(AssertionError):
            self.assert_control_order(script)

    def test_missing_ready_or_top_level_query_is_rejected(self):
        for line in ('wait_for_primary 30', QUERY):
            with self.subTest(line=line):
                script = (ROOT / 'tools/i11_pitr_container.sh').read_text()
                script = script.replace('\n' + line + '\n', '\n')
                with self.assertRaises(AssertionError):
                    self.assert_control_order(script)


if __name__ == '__main__':
    unittest.main()
