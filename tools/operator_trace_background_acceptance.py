"""Extend the existing S059 job; no workflow, deployment or live-host access."""
import json
import os
from pathlib import Path
import re
import subprocess

CHECKS = (
    'real_http', 'real_postgresql', 'real_writer_process', 'separate_worker_processes',
    'client_trace_rejected', 'replay_origin_preserved', 'origin_update_rejected',
    'http_duration_not_extended', 'retry_then_completed', 'writer_restart_preserved_attempt',
    'cancelled_worker_stale', 'cooperative_interruption_recorded',
    'killed_worker_missing_receipt_not_fabricated', 'lease_reclaim_kept_origin',
    'corrupt_source_rejected', 'writer_outage_did_not_fail_import', 'private_operator_browser',
)
LIMITATIONS = ('other_background_workers_tested', 'production_slo_proven',
               'independent_staging_tested', 'release_approved')
DATABASE = 'postgresql://tor_app:tor-reader-ci-app@127.0.0.1:5432/tor_ci'


def run(output: Path, database: str, password: str) -> dict:
    if os.environ.get('GITHUB_ACTIONS') != 'true' or database != DATABASE:
        raise RuntimeError('requires disposable S059 reader-browser PostgreSQL')
    runner = Path(os.environ['RUNNER_TEMP'])
    if not runner.is_absolute() or output != runner / 's059-pg' or output.is_symlink():
        raise RuntimeError('invalid S059 evidence directory')
    report_path = output / 'background-report.json'
    if report_path.exists():
        raise RuntimeError('refusing stale background evidence')
    sha = os.environ.get('GITHUB_SHA', '')
    if not re.fullmatch('[0-9a-f]{40}', sha):
        raise RuntimeError('exact source SHA required')
    env = dict(os.environ, TEST_DATABASE_URL=database,
               TOR_S059_BG_OUTPUT=str(output),
               TOR_TRACE_STORE_SOCKET=str(output / 'private' / 'background.sock'),
               TOR_OBS_OPERATOR_PASSWORD=password)
    subprocess.run([
        'cargo', 'test', '-p', 'tor-web', '--test', 's059_background_pg', '--',
        '--ignored', '--exact', 's059_http_import_retry_and_restart_evidence',
        '--nocapture', '--test-threads=1',
    ], env=env, check=True, timeout=600)
    if report_path.is_symlink() or not report_path.is_file() or report_path.stat().st_size > 32768:
        raise RuntimeError('missing or invalid background evidence')
    report = json.loads(report_path.read_text(encoding='utf-8'))
    if (not isinstance(report, dict) or report.get('scenario') != 'S059_background_import'
            or report.get('source_sha') != sha
            or report.get('source_store') != 'fault_controlled_private_fixture_not_s3'
            or any(report.get(key) is not True for key in CHECKS)
            or any(report.get(key) is not False for key in LIMITATIONS)):
        raise RuntimeError('incomplete background evidence or unsupported operational claim')
    return report
