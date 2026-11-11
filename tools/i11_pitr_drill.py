#!/usr/bin/env python3
"""Run real PG18/pgBackRest recovery in a mount-free isolated Docker sandbox."""
from __future__ import annotations

import io
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import uuid

DOCKER = ('docker', '--host=unix:///var/run/docker.sock')
ROOT = Path(__file__).resolve().parents[1]
CHECKS = (
    'source_removed', 'base_marker_restored', 'post_backup_wal_marker_restored',
    'post_target_marker_absent', 'wrong_key_rejected', 'unreachable_target_rejected',
    'role_denial_preserved', 'recovery_promoted',
)
LIMITATIONS = (
    'production_tested', 'offsite_tested', 'application_acl_tested',
    'rpo_slo_proven', 'rto_slo_proven', 'release_approved',
)
DOCKERFILE = b'''FROM postgres:18
USER root
RUN apt-get update && apt-get install -y --no-install-recommends pgbackrest python3 \\\n    && rm -rf /var/lib/apt/lists/*
COPY drill.sh /opt/tor-i11-drill.sh
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


def build_context(script: bytes) -> bytes:
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode='w') as tar:
        for name, content in (('Dockerfile', DOCKERFILE), ('drill.sh', script)):
            item = tarfile.TarInfo(name)
            item.size, item.mode = len(content), 0o644
            tar.addfile(item, io.BytesIO(content))
    return archive.getvalue()


def run_command(image: str, name: str) -> list[str]:
    return [*DOCKER, 'run', '--rm', '--name', name, '--network', 'none',
            '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
            '--read-only', '--tmpfs', '/tmp:rw,size=1073741824,mode=1777',
            '--memory', '1g', '--cpus', '2', '--pids-limit', '256',
            '--env', 'TOR_I11_DISPOSABLE_CONTAINER=yes', image]


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
    if type(report['schema_version']) is not int or report['schema_version'] != 1:
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


def main() -> int:
    if len(sys.argv) != 1:
        print('REFUSE: no arguments accepted', file=sys.stderr)
        return 2
    try:
        sha, destination = validate_environment(dict(os.environ))
    except ValueError as error:
        print('REFUSE: ' + str(error), file=sys.stderr)
        return 2
    name = 'tor-i11-pitr-' + uuid.uuid4().hex
    image = name + ':ci'
    env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'),
           'HOME': str(destination), 'LC_ALL': 'C'}
    context = build_context((ROOT / 'tools/i11_pitr_container.sh').read_bytes())
    try:
        subprocess.run([*DOCKER, 'build', '--tag', image, '-'], input=context,
                       env=env, check=True, timeout=600, stdout=subprocess.DEVNULL)
        result = subprocess.run(run_command(image, name), env=env, check=True,
                                timeout=300, capture_output=True, text=True)
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
            marker = re.search(r'^I11_PITR_FAILED line=([0-9]{1,4})$', stderr, re.M)
            if marker:
                print('I11_PITR_FAILED line=' + marker.group(1), file=sys.stderr)
            diagnostics = re.findall(
                r'^I11_PITR_DIAGNOSTIC (?:wrong-key|restore|bad-restore) [0-9]{3} [\x20-\x7e]{0,360}$',
                stderr, re.M)
            for diagnostic in diagnostics[:3]:
                print(diagnostic, file=sys.stderr)
        print('FAIL: isolated I-11 PITR drill; no release or SLO approval', file=sys.stderr)
        return 1
    finally:
        for command in ([*DOCKER, 'rm', '--force', '--volumes', name],
                        [*DOCKER, 'image', 'rm', image]):
            try:
                subprocess.run(command, env=env, timeout=20, check=False,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            except (OSError, subprocess.SubprocessError):
                pass


if __name__ == '__main__':
    raise SystemExit(main())
