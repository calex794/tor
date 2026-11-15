#!/usr/bin/env python3
"""TOR I-11: read-only PG18 WAL archive and offsite backup health probe.

Operator-installed only. A healthy probe does not prove PITR, an RPO target,
or independent S3 durability; these require separate restore drills.
"""

import datetime as dt
import json
import os
from pathlib import Path
import subprocess
import sys
import time

PGDATA = Path('/var/lib/postgresql/18/main')
PSQL = '/usr/lib/postgresql/18/bin/psql'
READY_LIMIT_SECONDS = 600
BACKUP_LIMIT_SECONDS = 36 * 3600
MIN_FREE_PERCENT = 25
ARCHIVE_TIMEOUT_SECONDS = 900
SQL = """
SELECT json_build_object(
  'data_directory', current_setting('data_directory'),
  'archive_mode', current_setting('archive_mode'),
  'archive_command', current_setting('archive_command'),
  'wal_level', current_setting('wal_level'),
  'archive_timeout_seconds',
    (SELECT setting::integer FROM pg_settings WHERE name='archive_timeout'),
  'archived_count', archived_count,
  'last_archived_time', last_archived_time,
  'last_failed_time', last_failed_time
)::text FROM pg_stat_archiver;
"""


class HealthError(Exception):
    pass


def checked(command, seconds):
    try:
        result = subprocess.run(command, capture_output=True, text=True,
                                timeout=seconds, check=False)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise HealthError('probe_command_unavailable') from exc
    if result.returncode != 0:
        # Never expose stderr; it can contain connection details or credentials.
        raise HealthError('probe_command_failed')
    return result.stdout


def timestamp(value):
    if value is None:
        return None
    if not isinstance(value, str):
        raise HealthError('invalid_archive_timestamp')
    try:
        parsed = dt.datetime.fromisoformat(value.replace('Z', '+00:00'))
    except ValueError as exc:
        raise HealthError('invalid_archive_timestamp') from exc
    if parsed.tzinfo is None:
        raise HealthError('timestamp_missing_timezone')
    return parsed.timestamp()


def evaluate(pg, repo, ready_ages, free_percent, now):
    """Pure fail-closed evaluation; inputs can be simulated in unit tests."""
    if not isinstance(pg, dict) or pg.get('data_directory') != str(PGDATA):
        raise HealthError('wrong_postgres_cluster')
    if pg.get('archive_mode') != 'on' or pg.get('wal_level') not in ('replica', 'logical'):
        raise HealthError('archiving_disabled')
    if pg.get('archive_command') != 'pgbackrest --stanza=tor archive-push %p':
        raise HealthError('unexpected_archive_command')
    if type(pg.get('archive_timeout_seconds')) is not int or not 0 < pg['archive_timeout_seconds'] <= ARCHIVE_TIMEOUT_SECONDS:
        raise HealthError('unsafe_archive_timeout')
    if not isinstance(repo, list):
        raise HealthError('invalid_backup_catalog')
    stanza = next((x for x in repo if isinstance(x, dict) and x.get('name') == 'tor'), None)
    if not stanza or not isinstance(stanza.get('status'), dict) or stanza['status'].get('code') != 0:
        raise HealthError('backup_repository_unhealthy')
    backups = stanza.get('backup')
    if not isinstance(backups, list) or not backups:
        raise HealthError('no_physical_backup')
    stops = [x.get('timestamp', {}).get('stop') for x in backups if isinstance(x, dict) and isinstance(x.get('timestamp'), dict)]
    if not stops or any(type(x) is not int for x in stops):
        raise HealthError('invalid_backup_timestamp')
    backup_age = now - max(stops)
    if backup_age < -300 or backup_age > BACKUP_LIMIT_SECONDS:
        raise HealthError('last_backup_too_old_or_future')
    if not isinstance(free_percent, (int, float)) or not 0 <= free_percent <= 100:
        raise HealthError('invalid_disk_free_percent')
    if free_percent < MIN_FREE_PERCENT:
        raise HealthError('low_wal_disk_space')
    if not isinstance(ready_ages, list) or any(not isinstance(x, (int, float)) or x < -5 for x in ready_ages):
        raise HealthError('invalid_pending_wal')
    if len(ready_ages) > 10000 or (ready_ages and max(ready_ages) > READY_LIMIT_SECONDS):
        raise HealthError('wal_archiver_backlog')
    archived = timestamp(pg.get('last_archived_time'))
    failed = timestamp(pg.get('last_failed_time'))
    if type(pg.get('archived_count')) is not int or pg['archived_count'] <= 0 or archived is None:
        raise HealthError('wal_archiving_never_confirmed')
    if archived > now + 300:
        raise HealthError('future_wal_archive_timestamp')
    if failed is not None and failed > archived:
        raise HealthError('last_wal_archive_operation_failed')
    age = max(0, int(now - archived))
    return {
        'status': 'ok',
        'pending_ready_segments': len(ready_ages),
        'oldest_ready_age_seconds': int(max(ready_ages, default=0)),
        'last_archived_age_seconds': age,
        'last_backup_age_seconds': int(max(0, backup_age)),
        'wal_disk_free_percent': round(free_percent, 1),
        # PostgreSQL need not archive an idle, unfinished WAL segment.
        'idle_rpo_unproven': not ready_ages and age > ARCHIVE_TIMEOUT_SECONDS,
        'rpo_slo_proven': False,
        'offsite_restore_proven': False,
    }


def collect_pending(now):
    status_dir = PGDATA / 'pg_wal' / 'archive_status'
    if not status_dir.is_dir() or status_dir.is_symlink():
        raise HealthError('wal_archive_status_missing_or_unsafe')
    ages = []
    try:
        for path in status_dir.glob('*.ready'):
            if path.is_symlink() or not path.is_file():
                raise HealthError('unsafe_wal_status_file')
            ages.append(now - path.stat().st_mtime)
            if len(ages) > 10000:
                raise HealthError('wal_archiver_backlog')
        disk = os.statvfs(PGDATA / 'pg_wal')
    except OSError as exc:
        raise HealthError('wal_disk_unreadable') from exc
    if disk.f_blocks <= 0:
        raise HealthError('invalid_wal_disk_size')
    return ages, disk.f_bavail * 100 / disk.f_blocks


def main():
    if len(sys.argv) != 1 or os.geteuid() != 0 or 'GITHUB_ACTIONS' in os.environ:
        raise HealthError('manual_root_host_only')
    if not Path(PSQL).is_file():
        raise HealthError('postgres18_client_missing')
    now = time.time()
    pg = json.loads(checked(['runuser', '-u', 'postgres', '--', PSQL,
                             '-X', '-At', '-h', '/var/run/postgresql', '-p', '5432',
                             '-d', 'tor', '-c', SQL], 15))
    # A remote S3-backed repository must be configured manually. No secrets are printed.
    repo = json.loads(checked(['runuser', '-u', 'postgres', '--', 'pgbackrest',
                               '--stanza=tor', '--output=json', 'info'], 35))
    ready_ages, free_percent = collect_pending(now)
    print('TOR_PITR_HEALTH ' + json.dumps(evaluate(pg, repo, ready_ages, free_percent, now),
                                          sort_keys=True, separators=(',', ':')))


if __name__ == '__main__':
    try:
        main()
    except (HealthError, ValueError, TypeError, json.JSONDecodeError) as exc:
        code = str(exc) if isinstance(exc, HealthError) else 'invalid_probe_data'
        print('TOR_PITR_HEALTH_CRITICAL ' + code, file=sys.stderr)
        sys.exit(2)
