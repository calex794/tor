"""DR-04 pure negative tests: health probe must fail closed without live PG/S3."""
import importlib.util
from pathlib import Path
import subprocess
import time
import unittest

SOURCE = Path(__file__).resolve().parents[2] / 'ops/check-pgbackrest-archive-health.py'
spec = importlib.util.spec_from_file_location('tor_dr04_monitor', SOURCE)
health = importlib.util.module_from_spec(spec)
spec.loader.exec_module(health)


class ArchiveHealthContract(unittest.TestCase):
    def setUp(self):
        self.now = time.time()
        self.pg = {
            'data_directory': '/var/lib/postgresql/18/main',
            'archive_mode': 'on', 'wal_level': 'replica',
            'archive_command': 'pgbackrest --stanza=tor archive-push %p',
            'archive_timeout_seconds': 900, 'archived_count': 5,
            'last_archived_time': '2026-11-14T00:00:00+00:00',
            'last_failed_time': None,
        }
        self.pg['last_archived_time'] = self.iso(self.now - 200)
        self.repo = [{'name': 'tor', 'status': {'code': 0},
                      'backup': [{'timestamp': {'stop': int(self.now - 300)}}]}]

    @staticmethod
    def iso(seconds):
        from datetime import datetime, timezone
        return datetime.fromtimestamp(seconds, timezone.utc).isoformat()

    def check(self, ready=None, free=55):
        return health.evaluate(self.pg, self.repo, ready if ready is not None else [], free, self.now)

    def test_ok_no_write_and_no_false_rpo_claim(self):
        r = self.check([25])
        self.assertEqual(r['status'], 'ok')
        self.assertFalse(r['rpo_slo_proven'])
        self.assertFalse(r['offsite_restore_proven'])
        source = SOURCE.read_text()
        for forbidden in ('pg_switch_wal(', 'ALTER SYSTEM', 'pgbackrest restore',
                          'pgbackrest backup', 'DROP DATABASE', 'systemctl ', 'aws s3 rm'):
            self.assertNotIn(forbidden, source)
        self.assertIn("'GITHUB_ACTIONS' in os.environ", source)

    def test_old_ready_wal_fails(self):
        with self.assertRaisesRegex(health.HealthError, 'wal_archiver_backlog'):
            self.check([601])

    def test_new_failure_after_archive_fails(self):
        self.pg['last_failed_time'] = self.iso(self.now - 10)
        with self.assertRaisesRegex(health.HealthError, 'last_wal_archive_operation_failed'):
            self.check()

    def test_failure_recovered_by_newer_archive_passes(self):
        self.pg['last_failed_time'] = self.iso(self.now - 500)
        self.assertEqual(self.check()['status'], 'ok')

    def test_disk_pressure_and_old_backup_fail(self):
        with self.assertRaisesRegex(health.HealthError, 'low_wal_disk_space'):
            self.check(free=24.9)
        self.repo[0]['backup'][0]['timestamp']['stop'] = int(self.now - 37 * 3600)
        with self.assertRaisesRegex(health.HealthError, 'last_backup_too_old_or_future'):
            self.check()

    def test_bad_archive_configuration_fails(self):
        self.pg['archive_command'] = '/bin/true'
        with self.assertRaisesRegex(health.HealthError, 'unexpected_archive_command'):
            self.check()
        self.pg['archive_command'] = 'pgbackrest --stanza=tor archive-push %p'
        self.pg['archive_timeout_seconds'] = 1800
        with self.assertRaisesRegex(health.HealthError, 'unsafe_archive_timeout'):
            self.check()

    def test_no_archives_or_backup_cannot_be_green(self):
        self.pg['archived_count'] = 0
        with self.assertRaisesRegex(health.HealthError, 'wal_archiving_never_confirmed'):
            self.check()
        self.pg['archived_count'] = 1
        self.repo[0]['status']['code'] = 1
        with self.assertRaisesRegex(health.HealthError, 'backup_repository_unhealthy'):
            self.check()

    def test_idle_is_not_marked_as_rpo_proof(self):
        self.pg['last_archived_time'] = self.iso(self.now - 7200)
        r = self.check()
        self.assertTrue(r['idle_rpo_unproven'])
        self.assertFalse(r['rpo_slo_proven'])

    def test_timestamp_and_future_backup_fail(self):
        self.pg['last_archived_time'] = '2026-11-14 01:00:00'
        with self.assertRaisesRegex(health.HealthError, 'timestamp_missing_timezone'):
            self.check()
        self.pg['last_archived_time'] = self.iso(self.now - 10)
        self.repo[0]['backup'][0]['timestamp']['stop'] = int(self.now + 1000)
        with self.assertRaisesRegex(health.HealthError, 'last_backup_too_old_or_future'):
            self.check()

    def test_probe_requires_host_and_refuses_ci(self):
        run = subprocess.run(['python3', str(SOURCE)], env={'GITHUB_ACTIONS': 'true'},
                             capture_output=True, text=True)
        self.assertEqual(run.returncode, 2)
        self.assertIn('manual_root_host_only', run.stderr)
        self.assertNotIn('TOR_PITR_HEALTH {', run.stdout)


if __name__ == '__main__':
    unittest.main()
