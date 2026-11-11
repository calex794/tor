"""PITR client failure diagnostics stay bounded and omit fixture data/secrets."""
from pathlib import Path
import importlib.util
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('pitr', ROOT / 'tools/i11_pitr_drill.py')
drill = importlib.util.module_from_spec(spec)
spec.loader.exec_module(drill)
client_diagnostics = drill.client_diagnostics


class Diagnostics(unittest.TestCase):
    def test_reports_connection_failure_without_socket_path(self):
        result = client_diagnostics('psql: error: connection to server on socket "/tmp/private/socket" failed: No such file or directory')
        self.assertEqual(result, ['I11_PITR_CLIENT psql: error: connection to server on socket [quoted] failed: No such file or directory'])

    def test_reports_query_error_without_identifier(self):
        self.assertEqual(client_diagnostics('ERROR:  relation "private_content" does not exist'),
                         ['I11_PITR_CLIENT ERROR:  relation [quoted] does not exist'])

    def test_omits_non_error_lines_and_explicit_secrets(self):
        self.assertEqual(client_diagnostics('SELECT secret FROM table\nDETAIL: private row\npassword=secret\nERROR: password=secret\nFATAL: repo1-cipher-pass=private'), [])

    def test_redacts_uris_keys_and_unquoted_paths(self):
        messages = client_diagnostics('ERROR: failed postgresql://user:credential@host/db\nERROR: failed /tmp/private/file\nFATAL: key ' + 'ab' * 32)
        self.assertNotIn('credential', ' '.join(messages))
        self.assertNotIn('/tmp/private', ' '.join(messages))
        self.assertNotIn('ab' * 32, ' '.join(messages))

    def test_bounds_size_count_and_removes_duplicates(self):
        self.assertEqual(len(client_diagnostics('ERROR: failure\n' * 20)), 1)
        self.assertEqual(len(client_diagnostics('\n'.join('ERROR: ' + str(i) for i in range(10)))), 3)
        self.assertLessEqual(len(client_diagnostics('ERROR: ' + 'x' * 1000)[0]), 376)

    def test_control_characters_cannot_escape_into_log(self):
        self.assertNotIn('\x1b', client_diagnostics('ERROR: \x1b[31mfailed')[0])


if __name__ == '__main__':
    unittest.main()
