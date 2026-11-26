#!/usr/bin/env python3
"""One-time root installation from operator-reviewed pinned helper sources."""
from __future__ import annotations

import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile

import adopt_edge_vhost as adoption
import coordinated_release as coordinated
import managed_edge_release as edge
import managed_systemd_release as unit
import release_bundle_v2 as bundle
import release_executor_v2 as executor

MODULES = (
    "release_bundle_v2.py", "managed_edge_release.py", "managed_systemd_release.py",
    "coordinated_release.py", "release_executor_v2.py", "adopt_edge_vhost.py",
    "install_release_v2.py",
)
HELPERS = Path("/usr/local/lib/tor-release-v2")
WRAPPER = Path("/usr/local/sbin/tor-one-click-release")
STATE = Path("/var/lib/tor-release")
MARKER = STATE / "release-format"
BACKUP = STATE / "v1-helper-backup"
V1_MARKER = b"# Installed by ops/bootstrap-one-click-release-vps.sh"
V2_MARKER = b"# TOR RELEASE PROTOCOL 2"


class InstallError(ValueError):
    pass


def atomic_write(path, data, mode):
    edge.trusted(path.parent, directory=True)
    fd, name = tempfile.mkstemp(prefix=".tor-install-", dir=path.parent)
    temp = Path(name)
    try:
        with os.fdopen(fd, "wb") as stream:
            os.fchmod(stream.fileno(), mode)
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp, path)
        edge.sync_dir(path.parent)
    finally:
        temp.unlink(missing_ok=True)


def source_files(source):
    edge.trusted(source, directory=True)
    if stat.S_IMODE(source.stat().st_mode) != 0o700:
        raise InstallError("approved source must be root-private")
    if {p.name for p in source.iterdir()} != {*MODULES, "wrapper.sh"}:
        raise InstallError("unexpected trusted helper source files")
    files = {name: edge.read_small(source / name, 1024 * 1024)
             for name in (*MODULES, "wrapper.sh")}
    for name in MODULES:
        compile(files[name], name, "exec")
    if V2_MARKER not in files["wrapper.sh"].splitlines():
        raise InstallError("unexpected v2 wrapper")
    return files


def install_helpers(directory, files):
    """Initial publish only. A retry must use exactly the approved sources."""
    if os.path.lexists(directory):
        info = edge.trusted(directory, directory=True)
        if stat.S_IMODE(info.st_mode) != 0o700:
            raise InstallError("unsafe installed helper directory")
        if {p.name for p in directory.iterdir()} != set(MODULES):
            raise InstallError("unexpected installed helper files")
        for name in MODULES:
            if (stat.S_IMODE(edge.trusted(directory / name).st_mode) != 0o600
                    or edge.read_small(directory / name, 1024 * 1024) != files[name]):
                raise InstallError("installed helper differs; explicit upgrade required")
        return
    with bundle.candidate(directory) as temporary:
        for name in MODULES:
            atomic_write(temporary / name, files[name], 0o600)
    edge.sync_dir(directory.parent)


def disable_v1(wrapper, backup, approved):
    """Replace old sudo target before Nginx adoption, fail closed until marker."""
    original = edge.read_small(wrapper, 1024 * 1024)
    if stat.S_IMODE(edge.trusted(wrapper).st_mode) != 0o755:
        raise InstallError("unsafe installed sudo target")
    if original == approved:
        if not os.path.lexists(backup):
            raise InstallError("missing v1 helper backup")
        if (stat.S_IMODE(edge.trusted(backup).st_mode) != 0o600
                or V1_MARKER not in edge.read_small(backup, 1024 * 1024).splitlines()):
            raise InstallError("invalid v1 backup")
        return
    if V1_MARKER not in original.splitlines() or V2_MARKER in original.splitlines():
        raise InstallError("unrecognized old release helper")
    if os.path.lexists(backup):
        if (stat.S_IMODE(edge.trusted(backup).st_mode) != 0o600
                or edge.read_small(backup, 1024 * 1024) != original):
            raise InstallError("v1 backup differs")
    else:
        atomic_write(backup, original, 0o600)
    atomic_write(wrapper, approved, 0o755)


def publish_marker(path):
    if os.path.lexists(path):
        raise InstallError("protocol marker already exists")
    atomic_write(path, b"2\n", 0o644)


def require_healthy_baseline(lock_fd, expected=None):
    """Never replace the legacy recovery route while the website is down.

    The selected app may still be a legacy release, so verify its live inode
    and both readiness endpoints without requiring a v2 manifest for it.
    The staged v2 candidate is verified separately by bootstrap.
    """
    executor.require_database_policy()
    store = executor.AppStore()
    app = executor.Application(store, edge.SIGNERS, lock_fd)
    sha = app.current()
    if expected is not None and sha != expected:
        raise InstallError("legacy release changed during protocol adoption")
    app.verify(sha)
    systemd = unit.Systemd()
    systemd.verify()
    snapshot = systemd.snapshot()
    unit.check_identity(snapshot)
    managed = unit.read_current(unit.DROPIN_DIR)
    if managed is not None:
        unit.check_effective(snapshot, snapshot, unit.policy(managed))
    app.verify(sha)
    return sha


def bootstrap(source, sha, vhost, pin):
    files = source_files(source)
    bundle.validate_sha(sha)
    if not re.fullmatch(r"[0-9a-f]{64}", pin):
        raise InstallError("explicit approved vhost hash required")
    if vhost.parent not in (Path("/etc/nginx/sites-available"), Path("/etc/nginx/conf.d")):
        raise InstallError("unsupported vhost path")
    for path in (Path("/usr/local"), HELPERS.parent, WRAPPER.parent,
                 STATE, Path("/etc/nginx"), vhost.parent):
        edge.trusted(path, directory=True)
    # Require existing sudoers/signers; don't create or broaden permissions.
    sudoers = Path("/etc/sudoers.d/tor-one-click-release")
    expected_rule = b"tor-deploy ALL=(root) NOPASSWD: /usr/local/sbin/tor-one-click-release *"
    if expected_rule not in edge.read_small(sudoers, 8192).splitlines():
        raise InstallError("existing restricted sudoers rule required")
    executor.command("visudo", "-c")
    edge.trusted(edge.SIGNERS)
    with executor.release_lock() as lock_fd:
        executor.require_main(sha)
        if os.path.lexists(MARKER):
            raise InstallError("v2 already configured; do not re-bootstrap")
        record = coordinated.Journal().load()
        if record is not None and record.state not in coordinated.TERMINAL:
            raise InstallError("unfinished transaction; root recovery required")
        baseline = require_healthy_baseline(lock_fd)
        # Bootstrap snapshot is separate from application releases: may adopt
        # even if that SHA exists as a legacy v1 app directory.
        staged = STATE / "bootstrap-bundles"
        staged.mkdir(mode=0o700, exist_ok=True)
        if stat.S_IMODE(edge.trusted(staged, directory=True).st_mode) != 0o700:
            raise InstallError("unsafe bootstrap staging root")
        target = staged / sha
        if not os.path.lexists(target):
            bundle.stage(executor.TOR / "incoming" / "v2" / sha,
                         target, sha, edge.SIGNERS)
        edge.trusted(target, directory=True)
        for name in bundle.FILES:
            edge.trusted(target / name)
        bundle.verify(target, sha, edge.SIGNERS)
        store = edge.EdgeStore(edge.ROOT)
        nginx = adoption.ProductionAdoption()
        with store.locked():
            # Validate the original or already adopted vhost before replacing
            # the fixed sudo target. Original backup is keyed by approved hash.
            _, actual = adoption.checked_vhost(vhost)
            if os.path.lexists(store.current):
                original = edge.read_small(store.root / "adoption-backups" /
                                           (pin + ".conf"), adoption.MAX_CONFIG)
                if (adoption.hash_bytes(original) != pin
                        or adoption.render(original, store.root) != actual
                        or store.active_sha() != sha):
                    raise InstallError("incomplete adoption requires operator review")
                nginx.test()
                nginx.probe(sha, store.releases / sha)
            else:
                if adoption.hash_bytes(actual) != pin:
                    raise InstallError("vhost differs from approved hash")
                adoption.render(actual, store.root)
                nginx.test()
                nginx.loaded(vhost)
            # Recheck after staging and vhost inspection, immediately before
            # replacing the legacy recovery route. No automatic v1 downgrade.
            require_healthy_baseline(lock_fd, baseline)
            install_helpers(HELPERS, files)
            disable_v1(WRAPPER, BACKUP, files["wrapper.sh"])
            # After this point an interrupted bootstrap blocks all regular
            # sudo releases until the root operator completes adoption.
            if not os.path.lexists(store.current):
                adoption.adopt(store, target, sha, edge.SIGNERS, vhost, pin, nginx)
            nginx.test()
            nginx.probe(sha, store.releases / sha)
            executor.require_main(sha)
            require_healthy_baseline(lock_fd, baseline)
            publish_marker(MARKER)
    print("TOR_V2_BOOTSTRAPPED " + sha)


def main():
    try:
        if os.geteuid() != 0 or len(sys.argv) != 4:
            raise InstallError("root and three explicit approvals required")
        source = Path(__file__).resolve().parent
        os.umask(0o077)
        os.environ.clear()
        os.environ.update(executor.SAFE_ENV)
        bootstrap(source, sys.argv[1], Path(sys.argv[2]), sys.argv[3])
        return 0
    except (ValueError, OSError, subprocess.SubprocessError) as exc:
        print("TOR_V2_BOOTSTRAP_REFUSED: " + type(exc).__name__, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
