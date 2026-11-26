#!/usr/bin/env python3
"""Fixed v2 release entry point for a future root-owned installed helper."""
from __future__ import annotations

from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import time
import uuid

import coordinated_release as coordinated
import managed_edge_release as edge
import managed_systemd_release as unit
import release_bundle_v2 as bundle
import release_ancestry as ancestry

TOR = Path("/opt/tor")
LOCK = Path("/run/lock/tor-one-click-release.lock")
STATE = Path("/var/lib/tor-release")
HOST = "pandaolate.ru"
APP = "tor-web.service"
PG = "postgresql@18-main.service"
TRACE = "tor-trace-store.service"
SAFE_ENV = {"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8", "HOME": "/root", "TOR_RELEASE_SIGNED_V2": "1"}


class ExecutorError(ValueError):
    pass


def command(*args, timeout=20, check=True):
    return subprocess.run(args, env=SAFE_ENV, capture_output=True, timeout=timeout,
                          check=check)


def require_main(sha):
    bundle.validate_sha(sha)
    result = command("curl", "--disable", "--fail", "--silent", "--show-error",
                     "--retry", "2", "--max-time", "15",
                     "--header", "Accept: application/vnd.github+json",
                     "https://api.github.com/repos/calex794/tor/git/ref/heads/main",
                     timeout=50)
    body = json.loads(result.stdout)
    if not isinstance(body, dict) or not isinstance(body.get("object"), dict):
        raise ExecutorError("invalid GitHub main metadata")
    if body.get("ref") != "refs/heads/main" or body["object"].get("sha") != sha:
        raise ExecutorError("candidate is no longer GitHub main")


def require_forward(sha, active):
    """Use the common immutable ancestry contract under the release lock."""
    return ancestry.verify_forward(ancestry.public_get, sha, active)


@contextmanager
def release_lock(path=LOCK):
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(fd)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid()
                or info.st_nlink != 1 or info.st_mode & 0o077):
            raise ExecutorError("unsafe release lock")
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield fd
    finally:
        os.close(fd)


class AppStore:
    def __init__(self, root=TOR):
        if not root.is_absolute() or root.resolve() != root:
            raise ExecutorError("application root must be canonical")
        self.root = root
        self.releases = root / "releases"
        self.link = root / "current"
        edge.trusted(root, directory=True)
        edge.trusted(self.releases, directory=True)

    def release(self, sha):
        bundle.validate_sha(sha)
        path = self.releases / sha
        edge.trusted(path, directory=True)
        info = edge.trusted(path / "tor-web")
        if not info.st_mode & stat.S_IXOTH:
            raise ExecutorError("web executable is not executable")
        return path

    def current(self):
        if not self.link.is_symlink() or self.link.lstat().st_uid != os.geteuid():
            raise ExecutorError("unexpected application release pointer")
        target = self.link.resolve(strict=True)
        if target.parent != self.releases or target != self.release(target.name):
            raise ExecutorError("application release pointer outside controlled releases")
        return target.name

    def switch(self, previous, candidate):
        if self.current() not in (previous, candidate):
            raise ExecutorError("application pointer changed outside transaction")
        target = self.release(previous)
        temporary = self.root / (".tor-restore-" + uuid.uuid4().hex)
        try:
            temporary.symlink_to(target)
            os.replace(temporary, self.link)
            edge.sync_dir(self.root)
        finally:
            temporary.unlink(missing_ok=True)

    def stage(self, source, sha, signers):
        bundle.validate_sha(sha)
        target = self.releases / sha
        if not os.path.lexists(target):
            bundle.stage(source, target, sha, signers)
        edge.trusted(target, directory=True)
        if stat.S_IMODE(target.stat().st_mode) != 0o755:
            raise ExecutorError("staged directory not traversable")
        for name in bundle.FILES:
            edge.trusted(target / name)
        bundle.verify(target, sha, signers)
        return target


def trace_inactive():
    result = command("systemctl", "show", TRACE, "--property=LoadState",
                     "--property=ActiveState", "--property=UnitFileState",
                     check=False)
    properties = dict(line.split("=", 1) for line in result.stdout.decode().splitlines()
                      if "=" in line)
    if (properties.get("ActiveState") != "inactive" or
            (properties.get("LoadState") != "not-found" and
             (result.returncode or properties.get("UnitFileState") not in
              ("disabled", "masked", "masked-runtime", "static")))):
        raise ExecutorError("active or enabled Trace Store requires separate migration")


def service_pid():
    command("systemctl", "is-active", "--quiet", APP)
    result = command("systemctl", "show", APP, "--property=MainPID", "--value")
    if not re.fullmatch(rb"[1-9][0-9]*\n?", result.stdout):
        raise ExecutorError("invalid web MainPID")
    return int(result.stdout)


def same_executable(pid, expected):
    if pid <= 1:
        return False
    live = Path("/proc") / str(pid) / "exe"
    return os.path.samestat(live.stat(), expected.stat())


def readiness():
    args = ("curl", "--disable", "--fail", "--silent", "--show-error",
            "--noproxy", "*", "--max-time", "5")
    for suffix in (("http://127.0.0.1:3000/readyz",),
                   ("--resolve", HOST + ":443:127.0.0.1", "https://" + HOST + "/readyz")):
        if command(*args, *suffix).stdout.strip() != b"ok":
            raise ExecutorError("database-backed readiness failed")


class Application:
    def __init__(self, store, signers, lock_fd):
        self.store, self.signers, self.lock_fd = store, signers, lock_fd

    def current(self):
        return self.store.current()

    def verify(self, sha):
        trace_inactive()
        if self.current() != sha:
            raise ExecutorError("application pointer differs from expected release")
        binary = self.store.release(sha) / "tor-web"
        pid = service_pid()
        if not same_executable(pid, binary):
            raise ExecutorError("live Rust process is not the selected binary")
        readiness()
        if service_pid() != pid or not same_executable(pid, binary):
            raise ExecutorError("web process changed during verification")

    def preflight(self, sha):
        require_forward(sha, self.current())
        path = self.store.release(sha)
        for name in bundle.FILES:
            edge.trusted(path / name)
        bundle.verify(path, sha, self.signers)
        trace_inactive()
        command("systemctl", "is-active", "--quiet", PG)
        if edge.read_small(STATE / "pg-activated", 64).strip() != b"PG_ACTIVATED":
            raise ExecutorError("verified PostgreSQL activation required")
        config = edge.read_small(unit.DROPIN_DIR / "20-postgres.conf", 8192)
        required = (
            b"Environment=TOR_DEMO_MODE=0",
            b"Environment=TOR_AUTH_ENABLED=1",
            b"Environment=TOR_PUBLIC_ORIGIN=https://pandaolate.ru",
            b"EnvironmentFile=/etc/tor/tor-app.env",
        )
        if not set(required).issubset(config.splitlines()):
            raise ExecutorError("production PostgreSQL policy missing")
        for name in bundle.SCRIPTS:
            command("bash", "-n", str(path / name))
        command("bash", str(path / "go-live-postgres-vps.sh"), "--check-runtime")

    def activate(self, sha):
        require_forward(sha, self.current())
        trace_inactive()
        path = self.store.release(sha)
        bundle.verify(path, sha, self.signers)
        # Descendants inherit the overall lock, so a surviving migration
        # cannot race a new update if the coordinator is interrupted.
        child = subprocess.Popen(
            ["bash", str(path / "go-live-postgres-vps.sh"), sha, "--forward"],
            env=SAFE_ENV, pass_fds=(self.lock_fd,),
        )
        try:
            status = child.wait()
        except KeyboardInterrupt:
            child.wait()
            raise
        if status:
            raise ExecutorError("signed application activation failed")
        self.verify(sha)

    def restore(self, previous, candidate):
        trace_inactive()
        self.store.switch(previous, candidate)
        command("systemctl", "daemon-reload")
        # Clear a failed candidate start limit on the fixed web unit.
        command("systemctl", "reset-failed", APP)
        command("systemctl", "restart", APP, timeout=90)
        # Retain the additive SQLx schema and all database contents.
        for _ in range(20):
            try:
                self.verify(previous)
                return
            except (ExecutorError, OSError, subprocess.SubprocessError):
                time.sleep(1)
        raise ExecutorError("previous Rust version not ready with retained schema")


def main(argv=None):
    args = sys.argv[1:] if argv is None else argv
    try:
        if os.geteuid() != 0:
            raise ExecutorError("root privileges required")
        if len(args) != 1:
            raise ExecutorError("one full SHA or root-only recovery required")
        recovery = args == ["--recover"]
        # The future sudo wrapper MUST accept a SHA only, never --recover.
        if not recovery:
            bundle.validate_sha(args[0])
        os.umask(0o077)
        os.environ.clear()
        os.environ.update(SAFE_ENV)
        with release_lock() as fd:
            app_store = AppStore()
            edge.trusted(edge.SIGNERS)
            edge.trusted(STATE, directory=True)
            edge_store = edge.EdgeStore(edge.ROOT)
            journal = coordinated.Journal()
            with edge_store.locked():
                app = Application(app_store, edge.SIGNERS, fd)
                coordinator = coordinated.Coordinator(
                    journal, edge_store, edge.ProductionNginx(),
                    unit.UnitPolicy(), unit.Systemd(), app,
                )
                if recovery:
                    record = coordinator.recover()
                    print("TOR_V2_RECOVERY " + (record.state if record else "no_transaction"))
                    return 0
                sha = args[0]
                previous = journal.load()
                if previous is not None and previous.state not in coordinated.TERMINAL:
                    raise ExecutorError("unfinished release; root recovery required")
                require_forward(sha, app.current())
                app.verify(app.current())
                target = app_store.stage(TOR / "incoming" / "v2" / sha, sha, edge.SIGNERS)
                if previous is not None and previous.state == "committed" and previous.candidate == sha:
                    coordinator.verify(previous)
                    print("TOR_V2_ALREADY_ACTIVE " + sha)
                    return 0
                record = coordinator.run(target, sha, edge.SIGNERS)
                if record.state != "committed":
                    raise ExecutorError("release did not commit")
                print("TOR_ONE_CLICK_RELEASE_V2_OK " + sha)
                return 0
    except (ValueError, OSError, subprocess.SubprocessError) as exc:
        # Do not log subprocess stderr or environment values.
        safe = str(exc) if isinstance(exc, ExecutorError) else type(exc).__name__
        print("TOR_V2_REFUSED: " + safe, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
