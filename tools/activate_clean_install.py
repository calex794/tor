#!/usr/bin/env python3
"""DEPLOY-06b: guarded first local startup on a prepared empty VPS.

Never used by UPDATE or RECOVER. No public bind, DNS, package installation,
TLS issuance, deploy-key installation, data deletion or v2 sudo activation.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import platform
import pwd
import re
import stat
import subprocess
import sys
import time

import adopt_edge_vhost as adoption
import install_release_v2 as installer
import managed_edge_release as edge
import managed_systemd_release as unit
import prepare_clean_install as clean
import release_bundle_v2 as bundle
import release_executor_v2 as executor

WORKSPACE = clean.PARENT / clean.NAME
ACTIVATION = "activation.json"
CERT = Path("/etc/letsencrypt/live/pandaolate.ru/fullchain.pem")
KEY = CERT.with_name("privkey.pem")
VHOST = Path(clean.DESTINATIONS["install-nginx.conf"])
ENABLED = Path("/etc/nginx/sites-enabled/pandaolate.conf")
PHASES = ("stage", "database", "edge", "application", "verify", "publish")


class Refused(ValueError):
    pass


def record_bytes(record, phase, state):
    return (json.dumps({
        "format": 1, "mode": "INSTALL_EMPTY_LOCAL", "sha": record["release_sha"],
        "target_machine_id": record["target_machine_id"],
        "manifest_sha256": record["manifest_sha256"],
        "phase": phase, "state": state, "public_accepted": False,
    }, sort_keys=True, indent=2) + "\n").encode()


class ActivationJournal:
    def __init__(self, directory):
        self.path = directory / ACTIVATION
        self.directory = directory

    def begin(self, record):
        if os.path.lexists(self.path):
            raise Refused("existing activation attempt; inspect and recover, never reinitialize")
        self.save(record, PHASES[0], "active")

    def save(self, record, phase, state):
        if phase not in PHASES or state not in ("active", "incomplete", "local_ready"):
            raise Refused("invalid activation phase")
        installer.atomic_write(self.path, record_bytes(record, phase, state), 0o600)


def run(record, journal, ops):
    """All prerequisites checked before any live mutation. No destructive undo."""
    ops.preflight()
    journal.begin(record)
    phase = PHASES[0]
    try:
        for phase, action in (
            ("stage", ops.stage), ("database", ops.database),
            ("edge", ops.configure_edge), ("application", ops.start),
            ("verify", ops.verify), ("publish", ops.publish),
        ):
            journal.save(record, phase, "active")
            action()
        journal.save(record, phase, "local_ready")
    except (Exception, KeyboardInterrupt) as failure:
        cleanup_ok = True
        try:
            if phase in ("application", "verify", "publish"):
                ops.stop_unaccepted()
        except Exception:
            cleanup_ok = False
        journal.save(record, phase, "incomplete")
        raise Refused("FIRST_START_INCOMPLETE: " + phase +
                      ("; web stopped or not started" if cleanup_ok
                       else "; service stop unverified")) from failure


def new_file(path, data, mode=0o644):
    """Never overwrite an existing live destination, including a symlink."""
    edge.trusted(path.parent, directory=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    with os.fdopen(fd, "wb") as output:
        os.fchmod(output.fileno(), mode)
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    edge.sync_dir(path.parent)


def read_prepared(workspace, sha, machine, pin, signers):
    if workspace.resolve(strict=True) != workspace:
        raise Refused("workspace must be canonical")
    if stat.S_IMODE(edge.trusted(workspace, directory=True).st_mode) != 0o700:
        raise Refused("prepared workspace must be private")
    path = workspace / "install-plan.json"
    if stat.S_IMODE(edge.trusted(path).st_mode) != 0o600:
        raise Refused("prepared plan must be private")
    record = clean.plan(sha, machine, pin)
    if json.loads(edge.read_small(path, 16384)) != record:
        raise Refused("prepared plan differs from approved inputs")
    if os.path.lexists(workspace / ACTIVATION):
        raise Refused("existing activation attempt; inspect and recover")
    clean.verified_source(workspace / "bundle", sha, signers, pin)
    return record


def query(sql, database="postgres"):
    # SQL text is a fixed program constant, no credential values or output logs.
    return executor.command("runuser", "-u", "postgres", "--",
                            "/usr/lib/postgresql/18/bin/psql", "-X", "-w",
                            "-h", "/var/run/postgresql", "-p", "5432",
                            "-d", database, "-v", "ON_ERROR_STOP=1", "-Atqc", sql).stdout.strip()


def check_bindings(output, ports):
    for line in output.splitlines():
        fields = line.split()
        if len(fields) < 5:
            raise Refused("unexpected ss listener inventory")
        local = fields[3]
        addr, _, port = local.rpartition(":")
        if port in ports and addr != "127.0.0.1":
            raise Refused("unexpected non-loopback service listener")


def check_empty_cluster():
    executor.command("systemctl", "is-active", "--quiet", executor.PG)
    clusters = executor.command("pg_lsclusters", "-h").stdout.decode("ascii").splitlines()
    if len(clusters) != 1 or clusters[0].split()[:4] != ["18", "main", "5432", "online"]:
        raise Refused("dedicated PostgreSQL 18/main on port 5432 required")
    if (not re.fullmatch(rb"18[0-9]{4}", query("SHOW server_version_num"))
            or query("SHOW listen_addresses") != b"127.0.0.1"
            or query("SHOW password_encryption") != b"scram-sha-256"):
        raise Refused("unexpected PostgreSQL version, listener or password policy")
    if query("SELECT count(*) FROM pg_database WHERE datname NOT IN ('postgres','template0','template1')") != b"0":
        raise Refused("existing user database; use recovery")
    if query("SELECT count(*) FROM pg_roles WHERE rolname IN ('tor_app','tor_migrator')") != b"0":
        raise Refused("existing TOR roles; use recovery")
    # A dedicated default database must also not contain user tables.
    if query("SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p','v','m','f','S')") != b"0":
        raise Refused("existing database objects; use recovery")
    check_bindings(executor.command("ss", "-H", "-ltn").stdout.decode(), {"5432"})


class Operations:
    def __init__(self, workspace, record, signers, lock_fd):
        self.source = workspace / "bundle"
        self.record = record
        self.sha = record["release_sha"]
        self.pin = record["manifest_sha256"]
        self.signers = signers
        self.lock_fd = lock_fd
        self.target = executor.TOR / "releases" / self.sha
        self.store = edge.EdgeStore(edge.ROOT)
        self.nginx = edge.ProductionNginx()

    def preflight(self):
        clean.verified_source(self.source, self.sha, self.signers, self.pin)
        executor.require_main(self.sha)
        # PGDATA created by the trusted OS package is the only exception to
        # the original prepare guard. No existing TOR state or recovery data.
        clean.absent(tuple(p for p in clean.MARKERS if p != "/var/lib/postgresql/18/main"))
        for path in (Path("/opt"), Path("/etc"), Path("/var/lib"),
                     Path("/etc/systemd/system"), VHOST.parent, ENABLED.parent,
                     Path("/etc/nginx/conf.d")):
            edge.trusted(path, directory=True)
        if any(ENABLED.parent.iterdir()) or any(Path("/etc/nginx/conf.d").iterdir()):
            raise Refused("disable unrelated/default Nginx vhosts explicitly first")
        user = pwd.getpwnam("tor-deploy")
        if user.pw_uid == 0 or user.pw_gid == 0:
            raise Refused("dedicated unprivileged tor-deploy account required")
        check_empty_cluster()
        executor.trace_inactive()
        if executor.command("systemctl", "is-active", "--quiet", executor.APP,
                            check=False).returncode == 0:
            raise Refused("existing running application")
        executor.command("systemctl", "is-active", "--quiet", "nginx.service")
        self.nginx.test()
        dump = executor.command("nginx", "-T").stdout.decode()
        active = "\n".join(line for line in dump.splitlines() if not line.lstrip().startswith("#"))
        if re.search(r"\bserver\s*\{", active):
            raise Refused("existing Nginx server block; clean host required")
        listeners = executor.command("ss", "-H", "-ltn").stdout.decode()
        if any(line.split()[3].rpartition(":")[2] in {"80", "443", "3000"}
               for line in listeners.splitlines() if len(line.split()) >= 5):
            raise Refused("existing web listener; clean host required")
        executor.command("openssl", "x509", "-in", str(CERT), "-noout", "-checkend", "86400")
        executor.command("openssl", "x509", "-in", str(CERT), "-noout", "-checkhost", executor.HOST)
        if not KEY.is_file():
            raise Refused("approved TLS private key required")
        for name in bundle.SCRIPTS:
            executor.command("bash", "-n", str(self.source / name))

    def stage(self):
        executor.TOR.mkdir(mode=0o755)
        executor.TOR.chmod(0o755)
        edge.trusted_dir(executor.TOR / "releases")
        executor.STATE.mkdir(mode=0o755)
        executor.STATE.chmod(0o755)
        edge.SIGNERS.parent.mkdir(mode=0o700)
        new_file(edge.SIGNERS, edge.read_small(self.signers, 16384))
        bundle.stage(self.source, self.target, self.sha, edge.SIGNERS)
        clean.verified_source(self.target, self.sha, edge.SIGNERS, self.pin)
        for path in (executor.TOR.parent, executor.STATE.parent, edge.SIGNERS.parent.parent):
            edge.sync_dir(path)

    def signed_script(self, name, *args):
        bundle.verify(self.target, self.sha, edge.SIGNERS)
        # Keep the overall lock inherited by the synchronous child, even if
        # this controller is killed. Never print shell stdout/stderr.
        child = subprocess.Popen(["bash", str(self.target / name), *args],
                                 env=executor.SAFE_ENV, pass_fds=(self.lock_fd,),
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            code = child.wait()
        except KeyboardInterrupt:
            child.wait()
            raise
        if code:
            raise Refused("signed empty-install database step refused")

    def database(self):
        check_empty_cluster()
        executor.require_main(self.sha)
        self.signed_script("init-tor-database.sh", "--install-empty")
        self.signed_script("apply-tor-migrations-vps.sh", self.sha, "--install-empty")
        version = query("SELECT max(version) FROM _sqlx_migrations WHERE success", "tor")
        if not re.fullmatch(rb"[0-9]{12}", version):
            raise Refused("canonical SQLx version not verified")
        new_file(executor.STATE / "schema-version", version + b"\n")

    def configure_edge(self):
        with self.store.locked():
            self.store.preserve_hsts("max-age=31536000")
            target = self.store.prepare(self.target, self.sha, edge.SIGNERS)
            self.store.switch_link(self.sha)
            data = edge.read_small(self.target / "install-nginx.conf", 262144)
            # The signed clean template is distinct from public adoption.
            # Reject even signed templates that bind outside localhost.
            listen = re.findall(rb"(?m)^\s*listen\s+([^;]+);", data)
            if len(listen) != 4 or any(not re.match(rb"127\.0\.0\.1:(80|443)(?: |$)", v) for v in listen):
                raise Refused("initial TLS vhost must be loopback-only")
            new_file(VHOST, data)
            try:
                ENABLED.symlink_to(VHOST)
                edge.sync_dir(ENABLED.parent)
                self.nginx.test()
                dump = executor.command("nginx", "-T").stdout.decode()
                if "# configuration file " + str(ENABLED) + ":" not in dump:
                    raise Refused("candidate vhost is not included")
                self.nginx.reload()
                self.nginx.probe(self.sha, target)
                snapshot = adoption.ProductionAdoption().snapshot("/catalog")
                if snapshot.status != 502:
                    raise Refused("offline edge acceptance failed")
                check_bindings(executor.command("ss", "-H", "-ltn").stdout.decode(),
                               {"80", "443", "3000", "5432"})
            except (Exception, KeyboardInterrupt):
                # Only remove our own exact newly created vhost/link.
                if ENABLED.is_symlink() and os.readlink(ENABLED) == str(VHOST):
                    ENABLED.unlink()
                    edge.sync_dir(ENABLED.parent)
                elif os.path.lexists(ENABLED):
                    raise Refused("edge cleanup unverified: unexpected enabled path")
                if edge.read_small(VHOST, 262144) != data:
                    raise Refused("edge cleanup unverified: vhost changed")
                VHOST.unlink()
                edge.sync_dir(VHOST.parent)
                self.nginx.test()
                self.nginx.reload()
                raise

    def start(self):
        executor.require_main(self.sha)
        bundle.verify(self.target, self.sha, edge.SIGNERS)
        unit.policy(edge.read_small(self.target / unit.SOURCE, 4096))
        for name, dest in clean.DESTINATIONS.items():
            if name == "install-nginx.conf":
                continue
            path = Path(dest)
            if path.parent == unit.DROPIN_DIR:
                edge.trusted_dir(path.parent)
            new_file(path, edge.read_small(self.target / name, 262144))
        link = executor.TOR / "current"
        link.symlink_to(self.target)
        edge.sync_dir(link.parent)
        executor.command("bash", str(self.target / "go-live-postgres-vps.sh"), "--check-runtime")
        executor.command("systemctl", "daemon-reload")
        unit.Systemd().verify()
        executor.command("systemctl", "start", executor.APP, timeout=90)

    def verify(self):
        app = executor.Application(executor.AppStore(), edge.SIGNERS, self.lock_fd)
        for _ in range(30):
            try:
                app.verify(self.sha)
                break
            except (ValueError, OSError, subprocess.SubprocessError):
                time.sleep(1)
        else:
            raise Refused("first real web process not ready")
        effective = unit.Systemd().snapshot()
        unit.check_identity(effective)
        if any(effective.get(k) != v for k, v in unit.policy(
                edge.read_small(self.target / unit.SOURCE, 4096)).items()):
            raise Refused("first web unit policy not effective")
        for path, parts in (
            ("/register", (b'name="csrf_token"', "Регистрац".encode())),
            ("/login", (b'name="csrf_token"', "Вход".encode())),
            ("/catalog", ()),
            ("/api/v1/books", ()),
        ):
            response = executor.command("curl", "--disable", "-fsS", "--noproxy", "*",
                                        "--max-time", "8", "--resolve",
                                        executor.HOST + ":443:127.0.0.1",
                                        "https://" + executor.HOST + path).stdout
            if not all(part in response for part in parts):
                raise Refused("initial HTTP auth/catalog acceptance failed")
            if path == "/api/v1/books" and "Первое знакомство".encode() in response:
                raise Refused("unexpected demo catalog")
        check_bindings(executor.command("ss", "-H", "-ltn").stdout.decode(),
                       {"80", "443", "3000", "5432"})
        self.nginx.probe(self.sha, self.store.releases / self.sha)
        app.verify(self.sha)

    def publish(self):
        executor.require_main(self.sha)
        executor.command("systemctl", "enable", executor.APP)
        new_file(executor.STATE / "pg-activated", b"PG_ACTIVATED\n")
        # No release-format marker, public DNS or public listen address here.

    def stop_unaccepted(self):
        # Do not stop PostgreSQL or delete/restore initialized DB or edge.
        # A failed first startup is left for explicit operator recovery.
        marker = executor.STATE / "pg-activated"
        if os.path.lexists(marker):
            if edge.read_small(marker, 64) != b"PG_ACTIVATED\n":
                raise Refused("unexpected activation marker")
            marker.unlink()
            edge.sync_dir(marker.parent)
        if os.path.lexists(Path(clean.DESTINATIONS["install-tor-web.service"])):
            executor.command("systemctl", "disable", "--now", executor.APP, timeout=90)
            if executor.command("systemctl", "is-active", "--quiet", executor.APP,
                                check=False).returncode == 0:
                raise Refused("unaccepted web service remains active")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-machine-id", required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--manifest-sha256", required=True)
    parser.add_argument("--signers", type=Path, required=True)
    parser.add_argument("--activate-local-empty", action="store_true", required=True)
    args = parser.parse_args()
    try:
        if os.geteuid() != 0 or "GITHUB_ACTIONS" in os.environ:
            raise Refused("root on approved independent VPS required")
        if platform.freedesktop_os_release().get("ID") != "ubuntu" or platform.freedesktop_os_release().get("VERSION_ID") != "24.04":
            raise Refused("expected Ubuntu 24.04")
        actual = Path("/etc/machine-id").read_text(encoding="ascii").strip()
        clean.validate_host_id(args.target_machine_id, actual)
        os.umask(0o077)
        os.environ.clear()
        os.environ.update(executor.SAFE_ENV)
        with executor.release_lock() as fd:
            record = read_prepared(WORKSPACE, args.sha, actual, args.manifest_sha256, args.signers)
            run(record, ActivationJournal(WORKSPACE), Operations(WORKSPACE, record, args.signers, fd))
        print("TOR_FIRST_LOCAL_READY " + args.sha + " public_accepted=false")
        return 0
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as exc:
        detail = str(exc) if isinstance(exc, Refused) else type(exc).__name__
        print("TOR_FIRST_START_REFUSED: " + detail, file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
