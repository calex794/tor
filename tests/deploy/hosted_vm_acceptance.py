#!/usr/bin/env python3
"""Real hosted-VM fixture. No operational CLI or production main override.

Only this test process pins synthetic release identities to a fixture
allowlist, never a production helper, deployed wrapper, or shell main gate.
The lab application activation boundary runs the real signed migration
script and real systemd restart. The production rollback/verify and all
other components are used unchanged.
"""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import time
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import activate_clean_install as first
import coordinated_release as coordinated
import install_release_v2 as transition
import managed_edge_release as edge
import managed_systemd_release as unit
import prepare_clean_install as clean
import release_bundle_v2 as bundle
import release_executor_v2 as executor

LAB = Path("/root/tor-deploy-lab")


def need(value, message):
    if not value:
        raise AssertionError(message)


def hosted_guard():
    sha = os.environ.get("GITHUB_SHA", "")
    need(os.geteuid() == 0 and os.environ.get("GITHUB_ACTIONS") == "true"
         and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
         and os.environ.get("RUNNER_OS") == "Linux"
         and os.environ.get("GITHUB_EVENT_NAME") in ("pull_request", "workflow_dispatch")
         and Path("/proc/1/comm").read_text().strip() == "systemd"
         and os.environ.get("GITHUB_WORKSPACE") == str(ROOT)
         and re.fullmatch(r"[0-9a-f]{40}", sha),
         "disposable GitHub-hosted VM only")
    need(stat.S_IMODE(edge.trusted(LAB, directory=True).st_mode) == 0o700
         and edge.read_small(LAB / "approved-checkout", 64).strip() == sha.encode(),
         "fresh guarded fixture marker required")
    need(not os.path.lexists(executor.TOR) and not os.path.lexists(first.WORKSPACE),
         "never run the lab on an existing installation")
    return sha


def fixture_release(label, sha, signers, key, *, broken=False):
    source = LAB / label
    bundle.build(ROOT, ROOT / "target/release", source, sha)
    if label != "A":
        asset = source / "fallback-game.js"
        with asset.open("ab") as output:
            output.write(("\n// disposable release generation " + label + "\n").encode())
        policy = source / unit.SOURCE
        policy.write_bytes(policy.read_bytes().replace(b"RestartSec=2s",
                           {"B": b"RestartSec=3s", "C": b"RestartSec=4s",
                            "D": b"RestartSec=5s"}[label]))
    if broken:
        # Deliberately signed startup failure, not a production Rust build.
        shutil.copyfile("/usr/bin/false", source / "tor-web")
        (source / "tor-web").chmod(0o755)
    hashes = {}
    for name in bundle.PAYLOAD:
        with (source / name).open("rb") as stream:
            hashes[name] = hashlib.file_digest(stream, "sha256").hexdigest()
    (source / bundle.MANIFEST).write_bytes(bundle.manifest_bytes(sha, hashes))
    executor.command("ssh-keygen", "-Y", "sign", "-f", str(key), "-n",
                     bundle.NAMESPACE, str(source / bundle.MANIFEST))
    bundle.verify(source, sha, signers)
    return source


def wait_app(app, sha, attempts=30):
    for _ in range(attempts):
        try:
            app.verify(sha)
            return
        except (ValueError, OSError, subprocess.SubprocessError):
            time.sleep(0.5)
    raise RuntimeError("fixture application did not reach expected readiness")


class LabApplication(executor.Application):
    def __init__(self, store, signers, lock_fd, *, crash=False):
        super().__init__(store, signers, lock_fd)
        self.crash = crash

    def activate(self, sha):
        executor.require_main(sha)
        target = self.store.release(sha)
        bundle.verify(target, sha, self.signers)
        # Exact existing migration script, real backup, SQLx and app role
        # checks. Production go-live current-main shell remains unmodified.
        child = subprocess.Popen(
            ["bash", str(target / "apply-tor-migrations-vps.sh"), sha],
            env=executor.SAFE_ENV, pass_fds=(self.lock_fd,),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        need(child.wait() == 0, "signed migration failed")
        self.store.switch(sha, self.current())
        executor.command("systemctl", "daemon-reload")
        executor.command("systemctl", "restart", executor.APP, timeout=90)
        wait_app(self, sha, attempts=16)
        if self.crash:
            # Actual abrupt process termination after both active pointers
            # changed; no exception handler or finally block can run here.
            os._exit(73)


def http(route, *, fail=True):
    args = ["curl", "--disable", "--silent", "--show-error", "--noproxy", "*",
            "--max-time", "6", "--resolve", executor.HOST + ":443:127.0.0.1"]
    if fail:
        args.append("--fail")
    else:
        args += ["--output", "/dev/null", "--write-out", "%{http_code}"]
    return executor.command(*args, "https://" + executor.HOST + route).stdout


def database_witness():
    return (
        first.query("SELECT count(*) || ':' || max(version) FROM _sqlx_migrations WHERE success", "tor"),
        first.query("SELECT count(*) FROM books WHERE source_identity='ci:deploy08a:retained' AND title='DEPLOY08A retained book'", "tor"),
        first.query("SELECT has_table_privilege('tor_app','public._sqlx_migrations','SELECT')::text || ',' || has_table_privilege('tor_app','public._sqlx_migrations','UPDATE')::text", "tor"),
    )


def secrets_witness():
    # Compare hashes only in memory, never publish credential material.
    return tuple(hashlib.sha256(edge.read_small(Path("/etc/tor") / name, 65536)).digest()
                 for name in ("tor-app.env", "tor-migrator.env"))


def run():
    checkout = hosted_guard()
    os.umask(0o077)
    # Same compiled checkout, separate synthetic identities for fault fixtures.
    shas = {label: hashlib.sha256((checkout + ":vm:" + label).encode()).hexdigest()[:40]
            for label in "ABCD"}
    key = LAB / "fixture-key"
    executor.command("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key))
    signers = LAB / "allowed-signers"
    signers.write_bytes(bundle.IDENTITY.encode() + b" " + key.with_suffix(".pub").read_bytes())
    signers.chmod(0o600)
    source = {label: fixture_release(label, sha, signers, key, broken=label == "C")
              for label, sha in shas.items()}
    key.unlink()  # no later signing during test
    key.with_suffix(".pub").unlink()
    pin = hashlib.sha256((source["A"] / bundle.MANIFEST).read_bytes()).hexdigest()
    machine = Path("/etc/machine-id").read_text().strip()
    record = clean.plan(shas["A"], machine, pin)
    # The hosted lab has package PGDATA, not an operator target. The CLI's
    # host/machine/PGDATA gate remains intact and is NOT disabled in code.
    clean.absent(tuple(p for p in clean.MARKERS if p != "/var/lib/postgresql/18/main"))
    clean.prepare(clean.PARENT, source["A"], shas["A"], signers, pin, machine)

    approved = {shas["A"]}
    def require_fixture_sha(sha):
        need(sha in approved, "unapproved fixture SHA")

    # Real main is unrelated to synthetic signed lab generations. Only the
    # lab substitutes the immutable ancestry check; the operator path uses
    # the common trusted GitHub ancestry module.
    def require_fixture_forward(sha, active):
        require_fixture_sha(sha)
        need(active in shas.values(), "unknown active fixture release")

    with patch.object(executor, "require_main", require_fixture_sha), \
         patch.object(executor, "require_forward", require_fixture_forward):
        with executor.release_lock() as fd:
            # Exercise actual prepared bundle/plan checks and every production
            # first-start phase (real PG init, migration, Nginx, systemd).
            restored_plan = first.read_prepared(first.WORKSPACE, shas["A"], machine, pin, signers)
            need(restored_plan == record, "prepared plan mismatch")
            first.run(restored_plan, first.ActivationJournal(first.WORKSPACE),
                      first.Operations(first.WORKSPACE, restored_plan, signers, fd))
            need(edge.read_small(executor.STATE / "pg-activated", 64) == b"PG_ACTIVATED\n",
                 "missing accepted PG activation marker")
            app_store = executor.AppStore()
            edge_store = edge.EdgeStore(edge.ROOT)
            nginx = edge.ProductionNginx()
            systemd = unit.Systemd()
            policy = unit.UnitPolicy()
            journal = coordinated.Journal()
            app = LabApplication(app_store, edge.SIGNERS, fd)
            app.verify(shas["A"])
            need(http("/catalog"), "missing first catalog")
            print("DEPLOY08A_REAL_FIRST_START_OK", flush=True)
            # The production transition guard is read-only and must accept
            # the healthy real app. This is not a wrapper installation.
            need(transition.require_healthy_baseline(fd) == shas["A"],
                 "healthy real baseline was not accepted")
            try:
                transition.require_healthy_baseline(fd, shas["B"])
            except transition.InstallError as exc:
                need("release changed" in str(exc), "wrong transition refusal")
            else:
                raise AssertionError("unexpected legacy release accepted")
            print("DEPLOY08C_REAL_HEALTHY_BASELINE_OK", flush=True)
            first.query("INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES ('ci:deploy08a:retained','ru','DEPLOY08A retained book','public','basic')", "tor")
            book = first.query("SELECT id FROM books WHERE source_identity='ci:deploy08a:retained'", "tor").decode()
            need(book.isdigit(), "fixture book not inserted")
            db = database_witness()
            credentials = secrets_witness()
            need(db[1:] == (b"1", b"true,false"), "incorrect baseline fixture rights")
            need(b"DEPLOY08A retained book" in http("/books/" + book), "fixture book unavailable")
            # Check actual autonomous edge with real Rust stopped.
            executor.command("systemctl", "stop", executor.APP)
            need(http("/catalog", fail=False) == b"502", "edge did not retain 502")
            need(http("/_tor-fallback/game.js"), "offline fallback asset unavailable")
            nginx.probe(shas["A"], edge_store.releases / shas["A"])
            # No mocked service status, process identity or readiness.
            # The stopped app must fail the real baseline preflight.
            try:
                transition.require_healthy_baseline(fd, shas["A"])
            except (executor.ExecutorError, subprocess.CalledProcessError):
                need(app.current() == shas["A"], "refusal changed active release")
                need(not os.path.lexists(transition.MARKER),
                     "refusal published protocol marker")
            else:
                raise AssertionError("stopped real app passed v2 transition guard")
            print("DEPLOY08C_REAL_OUTAGE_REFUSED", flush=True)
            executor.command("systemctl", "start", executor.APP)
            wait_app(app, shas["A"])
            need(transition.require_healthy_baseline(fd, shas["A"]) == shas["A"],
                 "recovered real baseline was not accepted")
            print("DEPLOY08C_REAL_RECOVERED_BASELINE_OK", flush=True)
            print("DEPLOY08A_REAL_BACKEND_OUTAGE_OK", flush=True)

            for label in "BCD":
                app_store.stage(source[label], shas[label], edge.SIGNERS)
            with edge_store.locked():
                def coordinator(application=app):
                    return coordinated.Coordinator(coordinated.Journal(), edge_store, nginx,
                                                   policy, systemd, application)

                approved.clear()
                approved.add(shas["B"])
                result = coordinator().run(app_store.release(shas["B"]), shas["B"], edge.SIGNERS)
                need(result.state == "committed", "B not committed")
                coordinator().verify(result)
                need(systemd.snapshot()["RestartUSec"] == "3s", "new real unit not loaded")
                need(database_witness() == db and secrets_witness() == credentials, "update changed witnesses")
                need(b"DEPLOY08A retained book" in http("/books/" + book), "B lost retained data")
                print("DEPLOY08A_SIGNED_REAL_UPDATE_OK", flush=True)

                approved.clear()
                approved.add(shas["C"])
                try:
                    coordinator().run(app_store.release(shas["C"]), shas["C"], edge.SIGNERS)
                except coordinated.ReleaseError as exc:
                    need("TOR_ROLLBACK_VERIFIED" in str(exc), "startup rollback not verified")
                else:
                    raise AssertionError("signed failing executable was accepted")
                failed = journal.load()
                need(failed.candidate == shas["C"] and failed.phase == "app"
                     and failed.state == "rolled_back", "failed candidate never reached real startup")
                coordinator().verify(failed, previous=True)
                need(database_witness() == db and secrets_witness() == credentials, "rollback lost data")
                print("DEPLOY08A_REAL_STARTUP_FAILURE_ROLLED_BACK", flush=True)

                approved.clear()
                approved.add(shas["D"])
                pid = os.fork()
                if pid == 0:
                    crash_app = LabApplication(app_store, edge.SIGNERS, fd, crash=True)
                    coordinator(crash_app).run(app_store.release(shas["D"]), shas["D"], edge.SIGNERS)
                    os._exit(74)
                _, status = os.waitpid(pid, 0)
                need(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 73,
                     "child did not exit at the injected real cutover boundary")
                interrupted = journal.load()
                need(interrupted.state == "active" and interrupted.phase == "app"
                     and interrupted.candidate == shas["D"], "missing durable interrupted intent")
                app.verify(shas["D"])
                nginx.probe(shas["D"], edge_store.releases / shas["D"])
                try:
                    coordinator().run(app_store.release(shas["D"]), shas["D"], edge.SIGNERS)
                except coordinated.ReleaseError as exc:
                    need("unfinished release" in str(exc), "incorrect interruption refusal")
                else:
                    raise AssertionError("second update ran over unfinished release")
                restored = coordinator().recover()
                need(restored.state == "rolled_back" and restored.previous_app == shas["B"],
                     "recovery did not return to prior B")
                coordinator().verify(restored, previous=True)
                need(database_witness() == db and secrets_witness() == credentials, "crash recovery lost state")
                need(b"DEPLOY08A retained book" in http("/books/" + book), "recovery lost retained book")
                print("DEPLOY08A_ABRUPT_PROCESS_EXIT_RECOVERED", flush=True)
    print("DEPLOY08A_VM_ACCEPTANCE_OK", flush=True)


if __name__ == "__main__":
    run()
