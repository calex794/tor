#!/usr/bin/env python3
"""Verify/stage an independently approved TOR release, entirely offline.

No install, binary execution, SQL, network, secrets, service or DNS changes.
Trust the verifier, manifest hash and signer hash obtained before the incident,
NOT the failed VPS, archive being checked or current GitHub main.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import hashlib
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile

BINARIES = ("tor-web", "tor-migrate", "tor-trace-store")
MANIFEST = "tor-release-manifest"
SIGNATURE = MANIFEST + ".sig"
FILES = frozenset(BINARIES + tuple(n + ".sha256" for n in BINARIES)
                  + (MANIFEST, SIGNATURE))
HEX_COMMIT = re.compile(r"[0-9a-f]{40}\Z")
HEX_SHA = re.compile(r"[0-9a-f]{64}\Z")
METADATA_LIMIT = 64 * 1024
BLOCK = 1024 * 1024
MAX_BINARY_BYTES = 2 * 1024 * 1024 * 1024


class Refused(ValueError):
    """Only static, non-sensitive failure descriptions."""


def require_digest(value):
    if not HEX_SHA.fullmatch(value) or value == "0" * 64:
        raise Refused("independent SHA256 pin required")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


@contextmanager
def directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        yield fd
    finally:
        os.close(fd)


@contextmanager
def regular_file(path, dir_fd=None):
    # O_NONBLOCK prevents a malicious FIFO from blocking before fstat.
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=dir_fd)
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise Refused("regular files required")
        with os.fdopen(fd, "rb", closefd=False) as handle:
            yield handle
    finally:
        os.close(fd)


def metadata(path, dir_fd=None):
    with regular_file(path, dir_fd) as handle:
        data = handle.read(METADATA_LIMIT + 1)
    if not data or len(data) > METADATA_LIMIT:
        raise Refused("empty or oversized release metadata")
    return data


def manifest_checksums(data, commit):
    """Exactly the canonical three-binary release manifest, in order."""
    lines = data.split(b"\n")
    if len(lines) != 5 or lines[-1] != b"":
        raise Refused("invalid release manifest layout")
    if lines[0] != ("TOR_RELEASE_SHA " + commit).encode("ascii"):
        raise Refused("release differs from independently approved commit")
    result = {}
    for line, name in zip(lines[1:4], BINARIES):
        if not re.fullmatch(rb"[0-9a-f]{64}  " + name.encode("ascii"), line):
            raise Refused("invalid signed checksum row")
        result[name] = line + b"\n"
    return result


def verify_signature(manifest, signature, signers):
    # Use the pinned signers bytes, not any signer file inside the bundle.
    # Fixed system verifier and argv; never execute any bundled executable.
    with tempfile.TemporaryDirectory(prefix="tor-recovery-signature-") as folder:
        root = Path(folder)
        allowed = root / "allowed_signers"
        sig = root / "manifest.sig"
        allowed.write_bytes(signers)
        sig.write_bytes(signature)
        result = subprocess.run(
            ["/usr/bin/ssh-keygen", "-Y", "verify", "-f", str(allowed),
             "-I", "tor-release", "-n", "tor-release", "-s", str(sig)],
            input=manifest, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            timeout=15, check=False,
        )
    if result.returncode != 0:
        raise Refused("release signature verification failed")


@contextmanager
def new_stage(root, commit):
    """Never overwrite or activate a release. Remove incomplete output on error."""
    if root is None:
        yield None
        return
    with directory(root) as parent:
        info = os.fstat(parent)
        if info.st_uid != os.geteuid() or info.st_mode & 0o022:
            raise Refused("staging root must be operator-owned and not writable by others")
        name = "tor-recovery-" + commit
        os.mkdir(name, mode=0o700, dir_fd=parent)
        child = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                        dir_fd=parent)
        try:
            yield child
            os.fsync(child)
            os.fsync(parent)
        except BaseException:
            # Only remove the newly created directory's known files.
            for filename in FILES:
                try:
                    os.unlink(filename, dir_fd=child)
                except FileNotFoundError:
                    pass
            os.rmdir(name, dir_fd=parent)
            raise
        finally:
            os.close(child)


@contextmanager
def new_file(name, stage):
    if stage is None:
        yield None
        return
    # Deliberately non-executable. A subsequent reviewed bootstrap must install.
    fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                 0o600, dir_fd=stage)
    with os.fdopen(fd, "wb") as handle:
        yield handle
        handle.flush()
        os.fsync(handle.fileno())


def verify_binary(name, source, stage, expected):
    digest = hashlib.sha256()
    count = 0
    with regular_file(name, source) as incoming, new_file(name, stage) as outgoing:
        while block := incoming.read(BLOCK):
            count += len(block)
            if count > MAX_BINARY_BYTES:
                raise Refused("release executable exceeds recovery size limit")
            digest.update(block)
            if outgoing is not None:
                outgoing.write(block)
    if count == 0 or digest.hexdigest() != expected:
        raise Refused("release executable missing or hash mismatch")


def verify_release(bundle, commit, manifest_pin, signers_file, signer_pin,
                   stage_root=None):
    if not HEX_COMMIT.fullmatch(commit):
        raise Refused("independently approved full release SHA required")
    require_digest(manifest_pin)
    require_digest(signer_pin)
    signers = metadata(signers_file)
    if sha256(signers) != signer_pin:
        raise Refused("independent signer pin mismatch")

    with directory(bundle) as source:
        if set(os.listdir(source)) != FILES:
            raise Refused("release bundle has missing or unexpected files")
        manifest = metadata(MANIFEST, source)
        if sha256(manifest) != manifest_pin:
            raise Refused("independent manifest pin mismatch")
        rows = manifest_checksums(manifest, commit)
        signature = metadata(SIGNATURE, source)
        verify_signature(manifest, signature, signers)
        small = {MANIFEST: manifest, SIGNATURE: signature}
        for name, row in rows.items():
            file = name + ".sha256"
            if metadata(file, source) != row:
                raise Refused("checksum file differs from signed manifest")
            small[file] = row

        with new_stage(stage_root, commit) as stage:
            for name, row in rows.items():
                verify_binary(name, source, stage, row[:64].decode("ascii"))
            if stage is not None:
                for name, data in small.items():
                    with new_file(name, stage) as handle:
                        handle.write(data)


def verify_release_v2(bundle, commit, manifest_pin, signers_file, signer_pin,
                      stage_root=None):
    """Reuse v2 exact-set signed import, with RECOVERY-01 independent pins.

    Verify-only uses a private temporary copy, removed at exit; persistent
    recovery stages are atomic and all files are deliberately mode 0600.
    Nothing is installed or executed, and no network or current-main check
    is involved. The legacy verifier remains usable on its own.
    """
    import import_offline_release as offline
    import release_bundle_v2 as v2

    if not HEX_COMMIT.fullmatch(commit):
        raise Refused("independently approved full release SHA required")
    require_digest(manifest_pin)
    require_digest(signer_pin)
    # Snapshot exactly the independently pinned signer bytes. Never verify
    # against a mutable signer file after checking its digest.
    signers = metadata(signers_file)
    if sha256(signers) != signer_pin:
        raise Refused("independent signer pin mismatch")

    if stage_root is not None:
        info = stage_root.lstat()
        if (not stage_root.is_absolute() or
                stage_root.resolve(strict=True) != stage_root or
                not stat.S_ISDIR(info.st_mode) or
                info.st_uid != os.geteuid() or info.st_mode & 0o077):
            raise Refused("v2 recovery staging root must be canonical and private")

    def import_into(parent, pinned_signers):
        try:
            offline.import_release(bundle, parent / "verified", commit,
                                   pinned_signers, manifest_pin)
        except offline.ImportError as exc:
            raise Refused("independent manifest pin or offline import refused") from exc
        except v2.BundleError as exc:
            raise Refused("signed v2 payload verification refused") from exc
        verified = parent / "verified"
        # No executable permissions in the published recovery stage. Any
        # later installation has to reverify and deliberately restore modes.
        for name in v2.FILES:
            path = verified / name
            with path.open("rb") as handle:
                os.fchmod(handle.fileno(), 0o600)
                os.fsync(handle.fileno())
            os.rename(path, parent / name)
        verified.rmdir()
        v2.verify(parent, commit, pinned_signers)

    with tempfile.TemporaryDirectory(prefix="tor-recovery-v2-") as temp:
        scratch = Path(temp)
        pinned = scratch / "pinned-signers"
        fd = os.open(pinned, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as handle:
            handle.write(signers)
        if stage_root is None:
            # The input directory is never modified. Verification needs
            # enough private scratch disk for one bounded complete bundle.
            candidate = scratch / "check"
            candidate.mkdir(mode=0o700)
            import_into(candidate, pinned)
        else:
            destination = stage_root / ("tor-recovery-" + commit)
            with v2.candidate(destination) as candidate:
                import_into(candidate, pinned)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--format", choices=("v1", "v2"), default="v1",
                        help="independently approved manifest format; no auto-downgrade")
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--manifest-sha256", required=True)
    parser.add_argument("--allowed-signers", type=Path, required=True)
    parser.add_argument("--signers-sha256", required=True)
    parser.add_argument("--stage-root", type=Path)
    args = parser.parse_args()
    try:
        verifier = verify_release if args.format == "v1" else verify_release_v2
        verifier(args.bundle, args.sha, args.manifest_sha256,
                 args.allowed_signers, args.signers_sha256, args.stage_root)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        detail = str(error) if isinstance(error, Refused) else "local verification/staging failed"
        print("TOR_RECOVERY_RELEASE_REFUSED: " + detail, file=sys.stderr)
        return 2
    result = "STAGED" if args.stage_root is not None else "VERIFIED"
    print(f"TOR_RECOVERY_RELEASE_{result} sha={args.sha} activation=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
