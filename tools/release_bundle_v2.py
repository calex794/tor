#!/usr/bin/env python3
"""DEPLOY-01: fixed, signed TOR release bundle (no VPS activation)."""

from __future__ import annotations

import argparse
from contextlib import contextmanager
import hashlib
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile

SHA = re.compile(r"[0-9a-f]{40}")
MANIFEST = "tor-release-manifest-v2"
SIGNATURE = MANIFEST + ".sig"
NAMESPACE = "tor-release-v2"
IDENTITY = "tor-release"
BINARIES = ("tor-web", "tor-migrate", "tor-trace-store")
EDGE = {
    "fallback-index.html": "ops/fallback/site/index.html",
    "fallback-style.css": "ops/fallback/site/style.css",
    "fallback-game.js": "ops/fallback/site/game.js",
    "fallback-plain.txt": "ops/fallback/site/plain.txt",
    "fallback-config.example.json": "ops/fallback/site/config.json",
    "nginx-http.conf": "ops/fallback/nginx-http.conf",
    "nginx-server.conf": "ops/fallback/nginx-server.conf",
    "nginx-proxy.conf": "ops/fallback/nginx-proxy.conf",
}
SYSTEMD = {"systemd-web-release.conf": "ops/systemd/40-tor-release.conf"}
SCRIPTS = {
    "go-live-postgres-vps.sh": "ops/go-live-postgres-vps.sh",
    "apply-tor-migrations-vps.sh": "ops/apply-tor-migrations-vps.sh",
}
CONFIGS = {**EDGE, **SYSTEMD, **SCRIPTS}
MIGRATOR_CHECKSUM = "tor-migrate.sha256"
PAYLOAD = (*BINARIES, *CONFIGS, MIGRATOR_CHECKSUM)
FILES = frozenset((*PAYLOAD, MANIFEST, SIGNATURE))
LIMITS = {name: 512 * 1024 * 1024 if name in BINARIES else 1024 * 1024
          for name in PAYLOAD}
LIMITS.update({MANIFEST: 4096, SIGNATURE: 16384})
CHUNK = 128 * 1024


class BundleError(ValueError):
    """No untrusted file content is included in error diagnostics."""


def validate_sha(sha: str) -> None:
    if not SHA.fullmatch(sha):
        raise BundleError("invalid release SHA")


@contextmanager
def open_directory(path: Path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        yield fd
    finally:
        os.close(fd)


@contextmanager
def open_regular(dir_fd: int, name: str, limit: int):
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                 dir_fd=dir_fd)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= limit:
            raise BundleError("file is empty, oversized or not regular: " + name)
        with os.fdopen(fd, "rb", closefd=False) as source:
            yield source
    finally:
        os.close(fd)


def consume(dir_fd: int, name: str, limit: int, output=None) -> str:
    """Bound reads even if an untrusted source changes size during copying."""
    digest = hashlib.sha256()
    total = 0
    with open_regular(dir_fd, name, limit) as source:
        while chunk := source.read(CHUNK):
            total += len(chunk)
            if total > limit:
                raise BundleError("file grew beyond limit: " + name)
            digest.update(chunk)
            if output is not None:
                output.write(chunk)
    if not total:
        raise BundleError("empty file: " + name)
    return digest.hexdigest()


def copy_entry(src_fd: int, source: str, dest: Path, limit: int, mode: int):
    with dest.open("xb") as output:
        consume(src_fd, source, limit, output)
        os.fchmod(output.fileno(), mode)
        output.flush()
        os.fsync(output.fileno())


def manifest_bytes(sha: str, hashes: dict[str, str]) -> bytes:
    validate_sha(sha)
    if set(hashes) != set(PAYLOAD) or any(
        not re.fullmatch(r"[a-f0-9]{64}", digest) for digest in hashes.values()
    ):
        raise BundleError("manifest requires exact payload and SHA-256 hashes")
    return ("TOR_RELEASE_MANIFEST 2\nTOR_RELEASE_SHA " + sha + "\n" +
            "".join(f"{hashes[name]}  {name}\n" for name in PAYLOAD)).encode("ascii")


def read_small(dir_fd: int, name: str) -> bytes:
    with open_regular(dir_fd, name, LIMITS[name]) as source:
        data = source.read(LIMITS[name] + 1)
    if not 0 < len(data) <= LIMITS[name]:
        raise BundleError("invalid metadata size: " + name)
    return data


@contextmanager
def candidate(destination: Path):
    """Caller owns a trusted parent and serializes release operations."""
    parent = destination.parent
    if parent.is_symlink() or not parent.is_dir():
        raise BundleError("candidate parent must be a real directory")
    info = parent.stat()
    if info.st_uid != os.geteuid() or info.st_mode & 0o022:
        raise BundleError("candidate parent must be owned by caller, not shared-writable")
    if os.path.lexists(destination):
        raise BundleError("candidate already exists")
    temporary = Path(tempfile.mkdtemp(prefix=".tor-bundle-", dir=parent))
    try:
        yield temporary
        # No untrusted writer may access parent; release executor holds its lock.
        if os.path.lexists(destination):
            raise BundleError("candidate already exists")
        with open_directory(temporary) as fd:
            os.fsync(fd)
        os.rename(temporary, destination)
        with open_directory(parent) as fd:
            os.fsync(fd)
    finally:
        if temporary.exists():
            shutil.rmtree(temporary)


def build(repo: Path, binaries: Path, destination: Path, sha: str) -> None:
    """Copy only the fixed allowlist from a trusted CI checkout."""
    validate_sha(sha)
    with candidate(destination) as temporary:
        for name in (*BINARIES, *CONFIGS):
            source = binaries / name if name in BINARIES else repo / CONFIGS[name]
            with open_directory(source.parent) as fd:
                copy_entry(fd, source.name, temporary / name, LIMITS[name],
                           0o755 if name in BINARIES else 0o644)
        with open_directory(temporary) as fd:
            digest = consume(fd, "tor-migrate", LIMITS["tor-migrate"])
        with (temporary / MIGRATOR_CHECKSUM).open("xb") as output:
            output.write(f"{digest}  tor-migrate\n".encode("ascii"))
            os.fchmod(output.fileno(), 0o644)
            output.flush()
            os.fsync(output.fileno())
        with open_directory(temporary) as fd:
            hashes = {name: consume(fd, name, LIMITS[name]) for name in PAYLOAD}
        with (temporary / MANIFEST).open("xb") as output:
            output.write(manifest_bytes(sha, hashes))
            os.fchmod(output.fileno(), 0o644)
            output.flush()
            os.fsync(output.fileno())


def verify(directory: Path, sha: str, signers: Path) -> None:
    """Verify a private, trusted staged directory. Never execute its files."""
    validate_sha(sha)
    with open_directory(directory) as fd:
        if set(os.listdir(fd)) != FILES:
            raise BundleError("unexpected or missing bundle entries")
        manifest = read_small(fd, MANIFEST)
        read_small(fd, SIGNATURE)
        hashes = {name: consume(fd, name, LIMITS[name]) for name in PAYLOAD}
        checksum = read_small(fd, MIGRATOR_CHECKSUM)
        if checksum != (hashes["tor-migrate"] + "  tor-migrate\n").encode("ascii"):
            raise BundleError("legacy migrator checksum differs from signed binary")
    if manifest != manifest_bytes(sha, hashes):
        raise BundleError("manifest does not match selected SHA and exact payload")
    result = subprocess.run(
        ["ssh-keygen", "-Y", "verify", "-f", str(signers), "-I", IDENTITY,
         "-n", NAMESPACE, "-s", str(directory / SIGNATURE)],
        input=manifest, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        timeout=15, check=False,
    )
    if result.returncode:
        raise BundleError("bundle signature verification failed")


def stage(source: Path, destination: Path, sha: str, signers: Path) -> None:
    """Copy from the potentially writable inbox before verifying signature/hash.

    The destination must be a new directory under a trusted caller-owned
    parent. The source directory is opened once; symlinks, special files and
    unexpected entries are rejected. The verified copy alone is published.
    """
    validate_sha(sha)
    with candidate(destination) as temporary, open_directory(source) as fd:
        if set(os.listdir(fd)) != FILES:
            raise BundleError("unexpected or missing bundle entries")
        for name in (*PAYLOAD, MANIFEST, SIGNATURE):
            copy_entry(fd, name, temporary / name, LIMITS[name],
                       0o755 if name in BINARIES else 0o644)
        verify(temporary, sha, signers)
        # The published verified binary must be traversable by tor-deploy.
        # Before verification the temporary directory stays private (0700).
        temporary.chmod(0o755)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    for action in ("build", "stage", "verify"):
        sub = actions.add_parser(action)
        sub.add_argument("--sha", required=True)
        if action == "build":
            sub.add_argument("--repo", type=Path, required=True)
            sub.add_argument("--binaries", type=Path, required=True)
        if action == "stage":
            sub.add_argument("--source", type=Path, required=True)
        if action in ("stage", "verify"):
            sub.add_argument("--signers", type=Path, required=True)
        sub.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.action == "build":
            build(args.repo, args.binaries, args.directory, args.sha)
        elif args.action == "stage":
            stage(args.source, args.directory, args.sha, args.signers)
        else:
            verify(args.directory, args.sha, args.signers)
    except (BundleError, OSError, subprocess.SubprocessError) as exc:
        detail = str(exc) if isinstance(exc, BundleError) else type(exc).__name__
        print("TOR_BUNDLE_REFUSED: " + detail, file=sys.stderr)
        return 1
    print("TOR_BUNDLE_" + args.action.upper() + "_OK " + args.sha)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
