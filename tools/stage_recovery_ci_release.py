#!/usr/bin/env python3
"""DEPLOY-07b: signed v2 release recovery fixture for the existing PG18 CI drill.

No operator/production mode. Only the synthetic GitHub Actions tor_ci
credentials and a freshly allocated test workspace are accepted by the CLI.
Actual compiled executables are copied with debug symbols stripped to keep
the fixture below the production bundle size cap; no fake payload executables.
The release signing key is an ephemeral CI fixture, never a deployment key.
"""
from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile

import import_offline_release as offline
import release_bundle_v2 as bundle
import tor_recovery_release as recovery

ROOT = Path(__file__).resolve().parents[1]
EXPECTED = {
    "GITHUB_ACTIONS": "true",
    "PGHOST": "127.0.0.1",
    "PGUSER": "postgres",
    "PGDATABASE": "postgres",
    "TEST_DATABASE_URL": "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci",
    "MIGRATOR_URL": "postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci",
}


class Refused(ValueError):
    pass


def check_environment(env, work):
    if any(env.get(name) != value for name, value in EXPECTED.items()):
        raise Refused("only disposable GitHub Actions PostgreSQL fixture allowed")
    if not env.get("RUNNER_TEMP"):
        raise Refused("runner temporary root required")
    root = Path(env["RUNNER_TEMP"]).resolve(strict=True)
    if (work.resolve(strict=True) != work or work.parent != root
            or not work.name.startswith("tor-pg-restore.")):
        raise Refused("unexpected disposable restore workspace")
    info = work.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise Refused("private restore workspace required")


def command(*args):
    # No credentials inherited by the local packaging utilities.
    return subprocess.run(args, cwd=ROOT, check=True, capture_output=True,
                          env={"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"},
                          timeout=90).stdout


def prepare(work, binary_root, sha):
    bundle.validate_sha(sha)
    if work.resolve(strict=True) != work or stat.S_IMODE(work.stat().st_mode) != 0o700:
        raise Refused("private canonical fixture workspace required")
    destination = work / "recovered-runtime"
    if os.path.lexists(destination):
        raise Refused("fixture runtime already exists")
    with tempfile.TemporaryDirectory(dir=work, prefix=".signed-restore-") as temp:
        scratch = Path(temp)
        binaries = scratch / "binaries"
        binaries.mkdir(mode=0o700)
        for name in bundle.BINARIES:
            source = binary_root / name
            info = source.lstat()
            if not stat.S_ISREG(info.st_mode) or not info.st_mode & 0o111:
                raise Refused("real built fixture executable required")
            command("/usr/bin/strip", "--strip-debug", "-o",
                    str(binaries / name), str(source))
        signed = scratch / "signed"
        bundle.build(ROOT, binaries, signed, sha)
        # The signing key is synthetic and held only in this private directory.
        key = scratch / "fixture-key"
        command("/usr/bin/ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key))
        signers = scratch / "approved-signers"
        signers.write_bytes(bundle.IDENTITY.encode() + b" " + key.with_suffix(".pub").read_bytes())
        signers.chmod(0o600)
        command("/usr/bin/ssh-keygen", "-Y", "sign", "-f", str(key),
                "-n", bundle.NAMESPACE, str(signed / bundle.MANIFEST))
        manifest_pin = hashlib.sha256((signed / bundle.MANIFEST).read_bytes()).hexdigest()
        signer_pin = hashlib.sha256(signers.read_bytes()).hexdigest()
        stage_root = scratch / "recovery"
        stage_root.mkdir(mode=0o700)
        # Use the real offline recovery path with both independent fixture pins.
        recovery.verify_release_v2(signed, sha, manifest_pin, signers,
                                   signer_pin, stage_root)
        staged = stage_root / ("tor-recovery-" + sha)
        if any(stat.S_IMODE((staged / name).stat().st_mode) != 0o600 for name in bundle.FILES):
            raise Refused("recovery stage must not be executable")
        # Separate explicit installation into this disposable private directory.
        # The original compiled binary is never executed by the restore drill.
        offline.import_release(staged, destination, sha, signers, manifest_pin)
        bundle.verify(destination, sha, signers)
        if hashlib.sha256((destination / bundle.MANIFEST).read_bytes()).hexdigest() != manifest_pin:
            raise Refused("installed fixture differs from selected release")
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--web-binary", type=Path, required=True)
    args = parser.parse_args()
    try:
        os.umask(0o077)
        check_environment(os.environ, args.work_dir)
        web = args.web_binary.absolute()
        if web.name != "tor-web":
            raise Refused("expected canonical compiled tor-web name")
        sha = command("/usr/bin/git", "rev-parse", "HEAD").decode("ascii").strip()
        prepare(args.work_dir, web.parent.resolve(strict=True), sha)
        print("DEPLOY07B_SIGNED_V2_RELEASE_RECOVERED " + sha)
        return 0
    except (ValueError, OSError, subprocess.SubprocessError) as exc:
        print("DEPLOY07B_FIXTURE_REFUSED: " + (str(exc) if isinstance(exc, Refused) else type(exc).__name__),
              file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
