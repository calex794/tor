#!/usr/bin/env python3
"""Run real PG18/pgBackRest recovery in a mount-free isolated Docker sandbox."""
from __future__ import annotations

import hashlib
import io
import json
import math
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tarfile
import tempfile
import uuid

DOCKER = ('docker', '--host=unix:///var/run/docker.sock')
ROOT = Path(__file__).resolve().parents[1]
MAX_FIXTURE_DUMP = 64 * 1024 * 1024
MAX_EXECUTABLE = 256 * 1024 * 1024
MINIO_RELEASE = 'minio.linux-amd64.RELEASE.2025-09-07T16-13-09Z'
HTTP_FIXTURE_LIMITS = {
    'rights.json': 16 * 1024,
    'pack.zst': 1024 * 1024,
    'pack.manifest.json': 1024 * 1024,
}
HTTP_EXECUTABLES = ('minio', 'http-reader-test')
CHECKS = (
    'source_removed', 'base_marker_restored', 'post_backup_wal_marker_restored',
    'post_target_marker_absent', 'wrong_key_rejected', 'unreachable_target_rejected',
    'role_denial_preserved', 'recovery_promoted',
    'utc_target_replay_paused', 'utc_target_transaction_boundary_verified',
    'utc_target_no_promotion_verified',
    'generated_startup_config_verified_and_used',
    'operator_paused_replay_helpers_verified',
    'tor_checkout_finance_replayed_from_wal',
    'original_finance_snapshot_digest_matched',
    'tor_refund_finance_and_revocation_replayed_from_wal',
    'tor_historical_paid_free_layer_rows_replayed_from_wal',
    'post_target_wallet_change_excluded',
    'physical_http_primary_cutoff_verified',
    'physical_free_layer_pack_http_verified',
)
LIMITATIONS = (
    'production_tested', 'offsite_tested', 'application_acl_tested',
    'rpo_slo_proven', 'rto_slo_proven', 'release_approved',
)
FIXTURE_MODULES = (
    'tor_recovery_host.py',
    'tor_recovery_pg.py',
    'tor_recovery_pg_prestart.py',
    'tor_recovery_pg_startup_config.py',
    'tor_recovery_pg_config_verify.py',
    'i11_pitr_prepare_config.py',
    'tor_recovery_pg_run.py',
    'i11_pitr_run_paused.py',
    'i11_finance_restore_facts.sql',
    'i11_paid_restore_facts.sql',
    'i11_historical_rights_restore_facts.sql',
    'i11_pitr_http_fixture.py',
    'i11_pitr_http.sh',
)
DOCKERFILE = b'''FROM postgres:18
USER root
RUN apt-get update && apt-get install -y --no-install-recommends pgbackrest python3 curl \\\n    && rm -rf /var/lib/apt/lists/*
COPY drill.sh /opt/tor-i11-drill.sh
COPY modules/ /opt/tor-i11-tools/
COPY bin/ /opt/tor-i11-bin/
USER postgres
ENTRYPOINT ["/bin/bash", "/opt/tor-i11-drill.sh"]
'''


def validate_environment(env: dict[str, str]) -> tuple[str, Path]:
    if env.get('GITHUB_ACTIONS') != 'true':
        raise ValueError('requires disposable GitHub Actions runner')
    sha = env.get('GITHUB_SHA', '')
    if not re.fullmatch(r'[0-9a-f]{40}', sha):
        raise ValueError('exact 40-character source SHA required')
    raw = env.get('RUNNER_TEMP', '')
    path = Path(raw)
    if not raw or not path.is_absolute() or path.is_symlink():
        raise ValueError('absolute real RUNNER_TEMP required')
    if not path.is_dir() or path.resolve() == Path('/'):
        raise ValueError('invalid RUNNER_TEMP')
    if env.get('DOCKER_HOST') not in (None, '', 'unix:///var/run/docker.sock'):
        raise ValueError('remote Docker daemons are forbidden')
    return sha, path.resolve()


def open_fixture_dump(raw: str, destination: Path):
    """Only the current disposable sealed-restore fixture, never a user archive."""
    path = Path(raw)
    if (not path.is_absolute() or path.name != 'restored.dump'
            or path.parent.parent != destination
            or not re.fullmatch(r'tor-dr-vault\.[A-Za-z0-9]{8}', path.parent.name)):
        raise ValueError('expected disposable sealed restore archive')
    parent = path.parent.lstat()
    if (not stat.S_ISDIR(parent.st_mode) or parent.st_uid != os.geteuid()
            or stat.S_IMODE(parent.st_mode) != 0o700):
        raise ValueError('expected private disposable archive directory')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid()
                or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o600
                or not 5 <= info.st_size <= MAX_FIXTURE_DUMP):
            raise ValueError('expected bounded private custom PostgreSQL archive')
        if os.read(fd, 5) != b'PGDMP':
            raise ValueError('expected PostgreSQL custom archive header')
        os.lseek(fd, 0, os.SEEK_SET)
        stream = os.fdopen(fd, 'rb')
    except BaseException:
        os.close(fd)
        raise
    return stream


def private_file(path: Path, limit: int, mode: int):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid()
                or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != mode
                or not 0 < info.st_size <= limit):
            raise ValueError('invalid bounded private HTTP fixture file')
        return os.fdopen(fd, 'rb')
    except BaseException:
        os.close(fd)
        raise


def load_http_executables(destination: Path, work: Path) -> dict[str, bytes]:
    with private_file(destination / MINIO_RELEASE, MAX_EXECUTABLE, 0o700) as f:
        minio = f.read()
    checksum_file = destination / (MINIO_RELEASE + '.sha256sum')
    with open(checksum_file, 'rb') as f:
        checksum = f.read(4096)
    expected = re.search(rb'[0-9a-f]{64}', checksum)
    if expected is None or hashlib.sha256(minio).hexdigest().encode() != expected[0]:
        raise ValueError('pinned MinIO executable digest differs')
    with private_file(work / 'http-reader-test', MAX_EXECUTABLE, 0o700) as f:
        reader = f.read()
    if not all(data.startswith(b'\x7fELF') for data in (minio, reader)):
        raise ValueError('expected locally built Linux HTTP fixture executables')
    return {'minio': minio, 'http-reader-test': reader}


def runtime_bundle(archive, work: Path):
    # All synthetic data and sessions travel only via runtime stdin.
    bundle = tempfile.TemporaryFile(dir=work)
    try:
        with tarfile.open(fileobj=bundle, mode='w') as tar:
            item = tarfile.TarInfo('fixture.dump')
            item.size = os.fstat(archive.fileno()).st_size
            item.mode = 0o600
            tar.addfile(item, archive)
            for name, limit in HTTP_FIXTURE_LIMITS.items():
                with private_file(work / name, limit, 0o600) as stream:
                    item = tarfile.TarInfo(name)
                    item.size = os.fstat(stream.fileno()).st_size
                    item.mode = 0o600
                    tar.addfile(item, stream)
        bundle.seek(0)
        return bundle
    except BaseException:
        bundle.close()
        raise


def build_context(script: bytes, modules: dict[str, bytes],
                  executables: dict[str, bytes]) -> bytes:
    if set(modules) != set(FIXTURE_MODULES):
        raise ValueError('exact allowlisted recovery fixture modules required')
    if set(executables) != set(HTTP_EXECUTABLES):
        raise ValueError('exact allowlisted HTTP fixture executables required')
    archive = io.BytesIO()
    entries = [('Dockerfile', DOCKERFILE, 0o644), ('drill.sh', script, 0o644)]
    entries.extend(('modules/' + name, modules[name], 0o644) for name in FIXTURE_MODULES)
    entries.extend(('bin/' + name, executables[name], 0o755) for name in HTTP_EXECUTABLES)
    with tarfile.open(fileobj=archive, mode='w') as tar:
        for name, content, mode in entries:
            item = tarfile.TarInfo(name)
            item.size, item.mode = len(content), mode
            tar.addfile(item, io.BytesIO(content))
    return archive.getvalue()


def run_command(image: str, name: str, finance_sha256: str) -> list[str]:
    if not re.fullmatch(r'[0-9a-f]{64}', finance_sha256) or finance_sha256 == '0' * 64:
        raise ValueError('independently retained fixture finance digest required')
    return [*DOCKER, 'run', '--rm', '--interactive', '--name', name, '--network', 'none',
            '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
            '--read-only', '--tmpfs', '/tmp:rw,size=1073741824,mode=1777',
            '--memory', '1g', '--cpus', '2', '--pids-limit', '256',
            '--env', 'TOR_I11_DISPOSABLE_CONTAINER=yes',
            '--env', 'TOR_I11_FINANCE_SHA256=' + finance_sha256, image]


def validate_report(raw: str) -> dict:
    if len(raw) > 32768:
        raise ValueError('unexpected oversized drill output')
    report = json.loads(raw)
    if not isinstance(report, dict):
        raise ValueError('report must be a JSON object')
    expected = {'schema_version', 'scope', 'postgres_version', 'pgbackrest_version',
                'backup_label', 'drill_elapsed_seconds', *CHECKS, *LIMITATIONS}
    if set(report) != expected:
        raise ValueError('incomplete or unexpected report fields')
    if type(report['schema_version']) is not int or report['schema_version'] != 7:
        raise ValueError('unsupported report schema')
    if report['scope'] != 'isolated_container_fixture':
        raise ValueError('invalid evidence scope')
    if not isinstance(report['postgres_version'], str) or not re.fullmatch(
            r'postgres \(PostgreSQL\) 18\.[^\r\n]{1,160}', report['postgres_version']):
        raise ValueError('PostgreSQL 18 required')
    if not isinstance(report['pgbackrest_version'], str) or not re.fullmatch(
            r'pgBackRest [0-9.]+', report['pgbackrest_version']):
        raise ValueError('invalid pgBackRest version')
    if not isinstance(report['backup_label'], str) or not re.fullmatch(
            r'[0-9]{8}-[0-9]{6}F', report['backup_label']):
        raise ValueError('full-backup label required')
    for key in CHECKS:
        if report[key] is not True:
            raise ValueError('unproven recovery check: ' + key)
    for key in LIMITATIONS:
        if report[key] is not False:
            raise ValueError('unsupported operational claim: ' + key)
    elapsed = report['drill_elapsed_seconds']
    if type(elapsed) not in (int, float) or not math.isfinite(elapsed) or not 0 < elapsed < 900:
        raise ValueError('invalid measured drill duration')
    return report


def client_diagnostics(stderr: str) -> list[str]:
    """Return SQL/connection failures with identifiers and possible secrets removed."""
    results = []
    for line in stderr[-32768:].splitlines():
        if not re.match(r'^(?:psql: error:|ERROR:|FATAL:|PANIC:)', line):
            continue
        if re.search(r'(?i)(?:cipher|password\s*[=:]|secret|token)', line):
            continue
        # These lines originate only from the fixed, network-free fixture.
        # Remove SQL identifiers, connection destinations and recovery keys.
        line = re.sub(r'"[^"\r\n]*"|\x27[^\x27\r\n]*\x27', '[quoted]', line)
        line = re.sub(r'[A-Za-z][A-Za-z0-9+.-]*://\S+', '[uri]', line)
        line = re.sub(r'/[^\s]*', '[path]', line)
        line = re.sub(r'\b[0-9a-fA-F]{32,}\b', '[redacted]', line)
        line = re.sub(r'[^\x20-\x7e]', '?', line)[:360]
        message = 'I11_PITR_CLIENT ' + line
        if message not in results:
            results.append(message)
        if len(results) == 3:
            break
    return results


def time_start_diagnostics(stderr: str) -> list[str]:
    """Bound synthetic fixture startup diagnostics; no raw PostgreSQL log."""
    rows = re.findall(
        r'^I11_PITR_TIME_START (?:time-start|time-pgctl|time-readiness|operator-replay) '
        r'[\x20-\x7e]{1,360}$', stderr[-32768:], re.M,
    )
    result = []
    for row in rows:
        if re.search(r'(?i)(?:cipher|password\s*[=:]|secret|token)', row):
            continue
        row = re.sub(r'[A-Za-z][A-Za-z0-9+.-]*://\S+', '[uri]', row)
        row = re.sub(r'\b[0-9a-fA-F]{32,}\b', '[redacted]', row)
        if row not in result:
            result.append(row)
        if len(result) == 8:
            break
    return result


def main() -> int:
    if (len(sys.argv) != 5 or sys.argv[1] != '--fixture-dump'
            or sys.argv[3] != '--finance-sha256'):
        print('REFUSE: exact disposable CI dump and finance digest required', file=sys.stderr)
        return 2
    try:
        sha, destination = validate_environment(dict(os.environ))
        finance_sha256 = sys.argv[4]
        if not re.fullmatch(r'[0-9a-f]{64}', finance_sha256) or finance_sha256 == '0' * 64:
            raise ValueError('invalid fixture finance digest')
        archive = open_fixture_dump(sys.argv[2], destination)
    except (OSError, ValueError):
        print('REFUSE: invalid disposable CI environment or archive', file=sys.stderr)
        return 2
    name = 'tor-i11-pitr-' + uuid.uuid4().hex
    image = name + ':ci'
    env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'),
           'HOME': str(destination), 'LC_ALL': 'C'}
    bundle = None
    try:
        work = Path(sys.argv[2]).parent
        bundle = runtime_bundle(archive, work)
        context = build_context(
            (ROOT / 'tools/i11_pitr_container.sh').read_bytes(),
            {name: (ROOT / 'tools' / name).read_bytes()
             for name in FIXTURE_MODULES},
            load_http_executables(destination, work),
        )
        subprocess.run([*DOCKER, 'build', '--tag', image, '-'], input=context,
                       env=env, check=True, timeout=600, stdout=subprocess.DEVNULL)
        # Stream the synthetic bundle at runtime. Never put data or session
        # tokens into the Docker image, build context, logs or a host bind mount.
        result = subprocess.run(run_command(image, name, finance_sha256), env=env, check=True,
                                stdin=bundle, timeout=420, capture_output=True, text=True)
        report = validate_report(result.stdout)
        report['source_sha'] = sha
        report['container_image_id'] = subprocess.check_output(
            [*DOCKER, 'image', 'inspect', '--format={{.Id}}', image],
            env=env, timeout=15, text=True).strip()
        if not re.fullmatch(r'sha256:[0-9a-f]{64}', report['container_image_id']):
            raise ValueError('invalid image digest')
        with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8',
                                         prefix='tor-i11-pitr-', suffix='.json',
                                         dir=destination, delete=False) as output:
            json.dump(report, output, indent=2, sort_keys=True, allow_nan=False)
            output.write('\n')
        print(json.dumps(report, sort_keys=True, allow_nan=False))
        print('I11_PITR_ISOLATED_DRILL_OK report=' + output.name)
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        if isinstance(error, subprocess.CalledProcessError):
            stderr = error.stderr or ''
            for diagnostic in client_diagnostics(stderr):
                print(diagnostic, file=sys.stderr)
            marker = re.search(r'^I11_PITR_FAILED line=([0-9]{1,4})$', stderr, re.M)
            if marker:
                print('I11_PITR_FAILED line=' + marker.group(1), file=sys.stderr)
            diagnostics = re.findall(
                r'^I11_PITR_DIAGNOSTIC (?:wrong-key|restore|bad-restore|time-restore) [0-9]{3} [\x20-\x7e]{0,360}$',
                stderr, re.M)
            for diagnostic in diagnostics[:4]:
                print(diagnostic, file=sys.stderr)
            for diagnostic in time_start_diagnostics(stderr):
                print(diagnostic, file=sys.stderr)
        print('FAIL: isolated I-11 PITR drill; no release or SLO approval', file=sys.stderr)
        return 1
    finally:
        archive.close()
        if bundle is not None:
            bundle.close()
        for command in ([*DOCKER, 'rm', '--force', '--volumes', name],
                        [*DOCKER, 'image', 'rm', image]):
            try:
                subprocess.run(command, env=env, timeout=20, check=False,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            except (OSError, subprocess.SubprocessError):
                pass


if __name__ == '__main__':
    raise SystemExit(main())
