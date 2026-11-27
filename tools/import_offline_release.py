#!/usr/bin/env python3
"""Verify an offline artifact and publish a private flat release directory.

Extract the GitHub artifact as an unprivileged user first. The extracted
directory is untrusted input: no arbitrary files, no execution, no extraction
as root. The signer and manifest pin must be supplied independently.
"""
from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path
import re
import stat
import subprocess
import sys

import managed_edge_release as edge
import release_bundle_v2 as bundle


class ImportError(ValueError):
    pass


def import_release(source, destination, sha, signers, pin):
    bundle.validate_sha(sha)
    if not re.fullmatch(r"[0-9a-f]{64}", pin):
        raise ImportError("independently approved manifest SHA-256 required")
    if (not signers.is_absolute() or signers.resolve(strict=True) != signers
            or not destination.is_absolute()
            or destination.parent.resolve(strict=True) != destination.parent):
        raise ImportError("canonical trusted output and signer paths required")
    edge.trusted(signers)
    edge.trusted(destination.parent, directory=True)
    with bundle.candidate(destination) as temporary:
        # Use the exact same bounded copy/verification path as the VPS.
        # Keep all bytes private until signature AND independent pin pass.
        copied = temporary / "verified"
        bundle.stage(source, copied, sha, signers)
        digest = hashlib.sha256(edge.read_small(copied / bundle.MANIFEST, 4096)).hexdigest()
        if digest != pin:
            raise ImportError("manifest differs from independently approved pin")
        # Publish a flat directory that prepare_clean_install can use directly.
        for name in bundle.FILES:
            os.rename(copied / name, temporary / name)
        copied.rmdir()
        bundle.verify(temporary, sha, signers)
        if stat.S_IMODE(temporary.stat().st_mode) != 0o700:
            raise ImportError("import directory must remain private")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--signers", required=True, type=Path)
    parser.add_argument("--manifest-sha256", required=True)
    args = parser.parse_args()
    try:
        os.umask(0o077)
        import_release(args.source, args.directory, args.sha, args.signers,
                       args.manifest_sha256)
        print("TOR_OFFLINE_IMPORT_OK " + args.sha)
        return 0
    except (ValueError, OSError, subprocess.SubprocessError) as exc:
        detail = str(exc) if isinstance(exc, ImportError) else type(exc).__name__
        print("TOR_OFFLINE_IMPORT_REFUSED: " + detail, file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
