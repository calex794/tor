"""Safety/contract tests; real PG18 recovery runs in the existing DR CI job."""
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('i11_pitr_drill', ROOT / 'tools/i11_pitr_drill.py')
drill = importlib.util.module_from_spec(spec)
spec.loader.exec_module(drill)


def valid_report():
    return {'schema_version': 1, 'scope': 'isolated_container_fixture',
            'postgres_version': 'postgres (PostgreSQL) 18.1',
            'pgbackrest_version': 'pgBackRest 2.57.0',
            'backup_label': '20261111-120000F', 'drill_elapsed_seconds': 2.5,
            **dict.fromkeys(drill.CHECKS, True), **dict.fromkeys(drill.LIMITATIONS, False)}


class RecoveryContract(unittest.TestCase):
    def test_accepts_complete_fixture_evidence(self):
        report = valid_report()
        self.assertEqual(drill.validate_report(json.dumps(report)), report)

    def test_every_check_is_required_and_boolean_true(self):
        for key in drill.CHECKS:
            for value in (False, None, 1, 'true'):
                with self.subTest(key=key, value=value):
                    report = valid_report()
                    report[key] = value
                    with self.assertRaises(ValueError):
                        drill.validate_report(json.dumps(report))

    def test_no_production_or_slo_claim(self):
        for key in drill.LIMITATIONS:
            for value in (True, None, 0, 'false'):
                with self.subTest(key=key, value=value):
                    report = valid_report()
                    report[key] = value
                    with self.assertRaises(ValueError):
                        drill.validate_report(json.dumps(report))

    def test_missing_field_fails(self):
        for key in valid_report():
            report = valid_report()
            del report[key]
            with self.subTest(key=key), self.assertRaises(ValueError):
                drill.validate_report(json.dumps(report))

    def test_unknown_fields_and_wrong_schema_fail(self):
        report = valid_report()
        report['secret'] = 'must not appear in output'
        with self.assertRaises(ValueError):
            drill.validate_report(json.dumps(report))
        for version in (True, 2, '1'):
            report = valid_report()
            report['schema_version'] = version
            with self.assertRaises(ValueError):
                drill.validate_report(json.dumps(report))

    def test_invalid_json_and_non_object(self):
        for content in ('', 'OK', '[]', 'null', 'true', '0', '{}\nsecret', ' ' * 32769):
            with self.subTest(content=content[:30]), self.assertRaises(ValueError):
                drill.validate_report(content)

    def test_duration_must_be_finite_positive_number(self):
        for value in (0, -1, True, '1', None, 900, float('inf'), float('nan')):
            report = valid_report()
            report['drill_elapsed_seconds'] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                drill.validate_report(json.dumps(report))

    def test_scope_and_versions_are_not_arbitrary_text(self):
        for key, value in (('scope', 'production'), ('postgres_version', 'postgres (PostgreSQL) 17.7'),
                           ('postgres_version', 'postgres (PostgreSQL) 18.1\nsecret'),
                           ('pgbackrest_version', 'secret'), ('backup_label', 'latest')):
            report = valid_report()
            report[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                drill.validate_report(json.dumps(report))


class Isolation(unittest.TestCase):
    def test_valid_ci_environment(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = {'GITHUB_ACTIONS': 'true', 'GITHUB_SHA': 'a' * 40, 'RUNNER_TEMP': tmp}
            self.assertEqual(drill.validate_environment(env), ('a' * 40, Path(tmp).resolve()))

    def test_refuses_local_and_remote_daemons(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = {'GITHUB_ACTIONS': 'true', 'GITHUB_SHA': 'a' * 40, 'RUNNER_TEMP': tmp}
            for key, value in (('GITHUB_ACTIONS', ''), ('GITHUB_ACTIONS', 'false'),
                               ('GITHUB_SHA', 'main'), ('GITHUB_SHA', 'A' * 40),
                               ('DOCKER_HOST', 'ssh://production'), ('DOCKER_HOST', 'tcp://127.0.0.1:2375'),
                               ('RUNNER_TEMP', '/'), ('RUNNER_TEMP', 'relative'), ('RUNNER_TEMP', '')):
                with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                    drill.validate_environment({**env, key: value})

    def test_refuses_symlink_and_missing_temp(self):
        with tempfile.TemporaryDirectory() as tmp:
            link = Path(tmp) / 'link'
            link.symlink_to(tmp, target_is_directory=True)
            for path in (link, Path(tmp) / 'missing'):
                with self.assertRaises(ValueError):
                    drill.validate_environment({'GITHUB_ACTIONS': 'true', 'GITHUB_SHA': 'a' * 40,
                                                'RUNNER_TEMP': str(path)})

    def test_runtime_has_no_ports_host_network_or_bind_mounts(self):
        command = drill.run_command('test-image', 'test-container')
        self.assertEqual(command[:2], ['docker', '--host=unix:///var/run/docker.sock'])
        self.assertEqual(command[command.index('--network') + 1], 'none')
        self.assertEqual(command[command.index('--cap-drop') + 1], 'ALL')
        self.assertIn('--read-only', command)
        self.assertIn('--memory', command)
        self.assertIn('--pids-limit', command)
        self.assertIn('--rm', command)
        for flag in ('--volume', '-v', '--mount', '--privileged', '--publish', '-p'):
            self.assertNotIn(flag, command)
        self.assertEqual(command.count('--env'), 1)
        self.assertEqual(command[command.index('--env') + 1], 'TOR_I11_DISPOSABLE_CONTAINER=yes')

    def test_build_context_has_only_two_fixed_files(self):
        content = drill.build_context(b'echo example\n')
        with tarfile.open(fileobj=io.BytesIO(content)) as archive:
            self.assertEqual(archive.getnames(), ['Dockerfile', 'drill.sh'])
            self.assertEqual(archive.extractfile('drill.sh').read(), b'echo example\n')
            self.assertTrue(all(member.isfile() for member in archive.getmembers()))
            self.assertIn(b'USER postgres', archive.extractfile('Dockerfile').read())

    def test_no_docker_invocation_outside_ci(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(sys, 'argv', ['drill']), \
                patch.object(drill.subprocess, 'run') as run, patch('sys.stderr', new_callable=io.StringIO):
            self.assertEqual(drill.main(), 2)
            run.assert_not_called()

    def test_unrequested_arguments_are_rejected(self):
        with patch.object(sys, 'argv', ['drill', '--production']), \
                patch.object(drill.subprocess, 'run') as run, patch('sys.stderr', new_callable=io.StringIO):
            self.assertEqual(drill.main(), 2)
            run.assert_not_called()

    def test_timeout_fails_and_cleanup_runs_without_secrets(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = {'GITHUB_ACTIONS': 'true', 'GITHUB_SHA': 'a' * 40, 'RUNNER_TEMP': tmp,
                   'PGPASSWORD': 'not-to-be-inherited', 'AWS_SECRET_ACCESS_KEY': 'private'}
            failure = subprocess.TimeoutExpired('docker', 600)
            with patch.dict(os.environ, env, clear=True), patch.object(sys, 'argv', ['drill']), \
                    patch.object(drill.subprocess, 'run', side_effect=[failure, None, None]) as run, \
                    patch('sys.stderr', new_callable=io.StringIO) as stderr:
                self.assertEqual(drill.main(), 1)
                self.assertEqual(run.call_count, 3)
                for call in run.call_args_list:
                    self.assertNotIn('PGPASSWORD', call.kwargs['env'])
                    self.assertNotIn('AWS_SECRET_ACCESS_KEY', call.kwargs['env'])
                self.assertNotIn('private', stderr.getvalue())
                self.assertEqual(list(Path(tmp).glob('*.json')), [])

    def test_existing_dr_job_invokes_pitr_without_an_extra_workflow(self):
        helper = (ROOT / 'tools/tor_dr_s3_backup_smoke.sh').read_text()
        self.assertEqual(helper.count('python3 tools/i11_pitr_drill.py'), 1)
        self.assertGreater(helper.index('python3 tools/i11_pitr_drill.py'),
                           helper.index("echo 'I11_S3_PG18_RESTORED_AXUM_ACL_OK'"))

    def test_script_is_syntactically_valid_and_has_positive_and_negative_targets(self):
        script = ROOT / 'tools/i11_pitr_container.sh'
        subprocess.run(['bash', '-n', str(script)], check=True)
        content = script.read_text()
        self.assertIn("pg_create_restore_point('tor_i11_target')", content)
        self.assertIn('tor_i11_nonexistent', content)
        self.assertLess(content.index('--type=full backup'), content.index("VALUES ('before_target')"))
        self.assertLess(content.index('rm -rf -- "$source_dir"'), content.index('--pg1-path="$restore_dir"'))
        self.assertIn('base_snapshot,before_target', content)
        self.assertIn('permission denied for table i11_recovery_markers', content)
        self.assertIn('recovery ended before configured recovery target was reached', content)
        self.assertEqual(content.count('-c hot_standby=off'), 2)


if __name__ == '__main__':
    unittest.main()
