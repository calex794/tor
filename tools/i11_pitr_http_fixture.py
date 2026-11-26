#!/usr/bin/env python3
"""Extract only fixed synthetic fixture members into the private container tmpfs."""
import os
from pathlib import Path
import re
import shutil
import sys
import tarfile

LIMITS = {
    'fixture.dump': 64 * 1024 * 1024,
    'rights.json': 16 * 1024,
    'pack.zst': 1024 * 1024,
    'pack.manifest.json': 1024 * 1024,
}
MAX_BUNDLE = 67 * 1024 * 1024


def unpack(work: Path) -> None:
    archive = work / 'runtime.tar'
    if not 0 < archive.stat().st_size <= MAX_BUNDLE:
        raise ValueError('invalid runtime bundle length')
    seen = set()
    with tarfile.open(archive, mode='r:') as tar:
        for member in tar:
            if (member.name not in LIMITS or member.name in seen
                    or not member.isfile() or member.issparse()
                    or not 0 < member.size <= LIMITS[member.name]):
                raise ValueError('unexpected runtime bundle member')
            seen.add(member.name)
            with tar.extractfile(member) as source, (work / member.name).open('xb') as dest:
                shutil.copyfileobj(source, dest)
    if seen != set(LIMITS):
        raise ValueError('incomplete runtime bundle')
    with (work / 'fixture.dump').open('rb') as stream:
        if stream.read(5) != b'PGDMP':
            raise ValueError('expected custom PostgreSQL archive')


def main() -> int:
    if (len(sys.argv) != 2 or os.geteuid() == 0 or not Path('/.dockerenv').is_file()
            or os.environ.get('TOR_I11_DISPOSABLE_CONTAINER') != 'yes'
            or not re.fullmatch(r'/tmp/tor-i11-pitr\.[A-Za-z0-9]{8}', sys.argv[1])):
        return 2
    work = Path(sys.argv[1])
    os.umask(0o077)
    try:
        unpack(work)
    except (OSError, ValueError, tarfile.TarError):
        print('I11_PITR_RUNTIME_BUNDLE_INVALID', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
