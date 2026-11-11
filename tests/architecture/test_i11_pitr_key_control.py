"""Prevent unrelated restore failures from passing the wrong-key test."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class RecoveryKeyControl(unittest.TestCase):
    def test_negative_and_positive_use_identical_restore_command(self):
        script = (ROOT / 'tools/i11_pitr_container.sh').read_text()
        negative = script.index('if restore_target "$work/wrong.conf"')
        no_cluster = script.index('[[ ! -e "$restore_dir/global/pg_control" ]]')
        positive = script.index('restore_target "$conf" >')
        query = script.index("SELECT pg_is_in_recovery()")
        success = script.index("'wrong_key_rejected': True")
        self.assertLess(negative, no_cluster)
        self.assertLess(no_cluster, positive)
        self.assertLess(positive, query)
        self.assertLess(query, success)
        command = script.split('restore_target() {', 1)[1].split('\n}', 1)[0]
        self.assertIn('--config="$1"', command)
        self.assertIn('--pg1-path="$restore_dir"', command)
        self.assertIn('--set="$label" --type=name --target=tor_i11_target', command)
        self.assertNotIn('|| true', command)
        self.assertIn('repo1-cipher-pass=deliberately-wrong-test-key/', script)


if __name__ == '__main__':
    unittest.main()
