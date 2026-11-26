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
import pwd
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
# Public immutable main ancestry for the full signed-shell test. These are
# fixture aliases, NOT claims to have built the historical commits. Every
# bundle contains this checkout's binaries/scripts and a temporary lab key.
# No ancestry override is installed for the real outage acceptance.
REAL_CHAIN = {
    "B": "85768a7b08e9fb12b5bc71943b9db17fceaf817d",
    "E": "3c433d846e89bec21cc57b3398f760499932f745",
    "F": "bb6e649e10f29af0d19d87eb564bfed0810e7bac",
    "G": "a198e6d6eff43fb0314d844b8977f7d05c02578e",
}


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


def fixture_release(label, sha, signers, key, *, broken=False, late_failure=False):
    source = LAB / label
    bundle.build(ROOT, ROOT / "target/release", source, sha)
    if label != "A":
        asset = source / "fallback-game.js"
        with asset.open("ab") as output:
            output.write(("\n// disposable release generation " + label + "\n").encode())
        policy = source / unit.SOURCE
        policy.write_bytes(policy.read_bytes().replace(b"RestartSec=2s",
                           {"B": b"RestartSec=3s", "C": b"RestartSec=4s",
                            "D": b"RestartSec=5s", "E": b"RestartSec=6s",
                            "F": b"RestartSec=7s", "G": b"RestartSec=8s"}[label]))
    if broken:
        # Deliberately signed startup failure, not a production Rust build.
        shutil.copyfile("/usr/bin/false", source / "tor-web")
        (source / "tor-web").chmod(0o755)
    if late_failure:
        # Signed test-only executable passes capability admission, then fails
        # startup after real migration/backup. Never used outside this lab.
        (source / "tor-web").write_bytes(
            b"#!/bin/sh\n"
            b'if [ "$#" = 1 ] && [ "$1" = "--release-capabilities" ]; then\n'
            b"  echo 'TOR_RELEASE_CAPABILITIES s3-covers=true s3-free=true s3-paid=true s3-chapter-import=true'\n"
            b"  exit 0\nfi\nexit 1\n")
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


def installed_executor(*args):
    # Use the actual installed root modules in a separate interpreter.
    # The synthetic fixture ancestry patch is not present in this process.
    result = subprocess.run(
        ["/usr/bin/python3", "-I", "-B", "-c",
         'import runpy, sys; sys.path.insert(0, "/usr/local/lib/tor-release-v2"); '
         'runpy.run_path("/usr/local/lib/tor-release-v2/release_executor_v2.py", run_name="__main__")',
         *args], env=executor.SAFE_ENV, capture_output=True, timeout=240)
    # Only fixed diagnostics from fixture output; never publish raw stderr.
    for line in (result.stdout + b"\n" + result.stderr).splitlines():
        if re.fullmatch(rb"(?:TOR_V2_OUTAGE_REPAIRED|TOR_PRODUCTION_OK) [0-9a-f]{40}", line):
            print(line.decode(), flush=True)
        elif line in (b"TOR_RECOVERY_SCHEMA_READY: web remains stopped; activation still required",
                      b"TOR_V2_RECOVERY outage_stopped",
                      b"TOR_V2_REFUSED: TOR_OUTAGE_RESTORED_STOPPED: website remains stopped",
                      b"TOR_V2_REFUSED: TOR_ROLLBACK_UNVERIFIED: root inspection required"):
            print(line.decode(), flush=True)
        elif line.startswith(b"REFUSE_RELEASE_ANCESTRY:"):
            print("DEPLOY08D_ANCESTRY_REFUSED", flush=True)
    return result


def approve_outage(previous, candidate):
    first.new_file(executor.OUTAGE_APPROVAL,
                   (previous + "\n" + candidate + "\n").encode("ascii"), 0o600)


def repair_witness(db, credentials, book, *, stopped, previous, candidate=None):
    app = executor.AppStore()
    need(app.current() == previous, "unexpected outage app pointer")
    need(edge.EdgeStore(edge.ROOT).active_sha() == previous, "unexpected outage edge pointer")
    need(database_witness() == db and secrets_witness() == credentials,
         "outage repair changed database or credentials")
    need(not os.path.lexists(executor.OUTAGE_APPROVAL), "outage approval was not consumed")
    record = coordinated.Journal().load()
    need(record.mode == "outage", "missing outage journal")
    if stopped:
        need(record.state == "outage_stopped" and record.candidate == candidate,
             "stopped restoration not durably recorded")
        executor.require_stopped_web()
        need(http("/catalog", fail=False) == b"502", "offline fallback missing")
        need(http("/_tor-fallback/game.js"), "offline asset missing")
    else:
        need(record.state == "committed" and record.candidate == previous,
             "healthy repair was not durably committed")
        executor.Application(app, edge.SIGNERS, -1).verify(previous)
        need(b"DEPLOY08A retained book" in http("/books/" + book), "repair lost retained book")


def real_outage_acceptance(source, shas, db, credentials, book):
    # The guarded disposable first-install lab has no legacy wrapper. Install
    # exactly the reviewed private modules and fixture marker, NOT the actual
    # v1 adoption/bootstrap. That remains a separate acceptance requirement.
    need(executor.AppStore().current() == shas["B"], "unexpected real-chain baseline")
    for prev, candidate in zip("BEF", "EFG"):
        executor.require_forward(shas[candidate], shas[prev])
    files = {name: (ROOT / "tools" / name).read_bytes() for name in transition.MODULES}
    transition.install_helpers(transition.HELPERS, files)
    transition.publish_marker(transition.MARKER)
    executor.require_v2_marker()
    # Real staging from a deployment-user-owned inbox.
    deploy = pwd.getpwnam("tor-deploy")
    incoming = executor.TOR / "incoming"
    inbox = incoming / "v2"
    for directory in (incoming, inbox):
        directory.mkdir(mode=0o755)
        directory.chmod(0o755)
        os.chown(directory, deploy.pw_uid, deploy.pw_gid)
    for label in "EFG":
        target = inbox / shas[label]
        shutil.copytree(source[label], target)
        for item in (target, *target.iterdir()):
            os.chown(item, deploy.pw_uid, deploy.pw_gid)

    # A valid approval never authorizes taking a healthy site offline.
    approve_outage(shas["B"], shas["E"])
    before = coordinated.Journal().load()
    result = installed_executor("--repair-outage", shas["E"])
    need(result.returncode != 0 and executor.OUTAGE_APPROVAL.exists(),
         "healthy app accepted or consumed outage approval")
    need(coordinated.Journal().load() == before and
         executor.AppStore().current() == shas["B"], "healthy refusal changed state")
    executor.OUTAGE_APPROVAL.unlink()
    edge.sync_dir(executor.OUTAGE_APPROVAL.parent)
    executor.command("systemctl", "stop", executor.APP)
    executor.require_stopped_web()
    executor.require_outage_edge()
    # The ordinary sudo target and the explicit root path both fail closed.
    result = installed_executor(shas["E"])
    need(result.returncode != 0, "ordinary update accepted stopped app")
    wrapper = ROOT / "ops/one-click-release-v2-wrapper.sh"
    result = subprocess.run(["bash", str(wrapper), "--repair-outage", shas["E"]],
                            env=executor.SAFE_ENV, capture_output=True, timeout=10)
    need(result.returncode == 2, "ordinary sudo wrapper accepted outage arguments")
    result = installed_executor("--repair-outage", shas["E"])
    need(result.returncode != 0, "outage accepted without approval")
    approve_outage(shas["B"], shas["F"])
    result = installed_executor("--repair-outage", shas["E"])
    need(result.returncode != 0 and executor.OUTAGE_APPROVAL.exists(),
         "wrong exact SHA approval accepted")
    need(coordinated.Journal().load() == before and
         executor.AppStore().current() == shas["B"] and
         edge.EdgeStore(edge.ROOT).active_sha() == shas["B"], "refusal changed live state")
    executor.OUTAGE_APPROVAL.unlink()
    edge.sync_dir(executor.OUTAGE_APPROVAL.parent)
    print("DEPLOY08D_REAL_UNAPPROVED_REFUSED", flush=True)

    backups = set(Path("/var/backups/tor").glob("tor-before-*.dump"))
    approve_outage(shas["B"], shas["E"])
    result = installed_executor("--repair-outage", shas["E"])
    need(result.returncode != 0 and
         b"TOR_V2_REFUSED: TOR_OUTAGE_RESTORED_STOPPED:" in result.stderr,
         "signed failing repair did not report stopped restoration")
    need(b"TOR_RECOVERY_SCHEMA_READY:" in result.stdout,
         "failing repair never reached actual recovery migration")
    new_backups = set(Path("/var/backups/tor").glob("tor-before-*.dump")) - backups
    need(len(new_backups) == 1, "missing actual pre-migration backup")
    executor.command("pg_restore", "-l", str(next(iter(new_backups))))
    repair_witness(db, credentials, book, stopped=True,
                   previous=shas["B"], candidate=shas["E"])
    need(unit.Systemd().snapshot()["RestartUSec"] == "3s", "old unit policy not restored")
    result = installed_executor("--repair-outage", shas["E"])
    need(result.returncode != 0, "consumed outage approval was replayed")
    print("DEPLOY08D_REAL_FAILED_REPAIR_STOPPED", flush=True)

    approve_outage(shas["B"], shas["F"])
    result = installed_executor("--repair-outage", shas["F"])
    need(result.returncode == 0 and
         ("TOR_V2_OUTAGE_REPAIRED " + shas["F"]).encode() in result.stdout,
         "actual signed repair did not succeed")
    need(b"TOR_RECOVERY_SCHEMA_READY:" in result.stdout, "missing real recovery migration")
    repair_witness(db, credentials, book, stopped=False, previous=shas["F"])
    need(unit.Systemd().snapshot()["RestartUSec"] == "7s", "new unit policy not applied")
    print("DEPLOY08D_REAL_SIGNED_REPAIR_OK", flush=True)

    # Crash the root executor after the actual signed shell has started and
    # verified G, before the coordinator can durably commit. No shell, Git
    # proof, service, database or verification boundary is mocked.
    executor.command("systemctl", "stop", executor.APP)
    approve_outage(shas["F"], shas["G"])
    class CrashAfterRepair(executor.Application):
        def activate_outage(self, sha):
            super().activate_outage(sha)
            os._exit(73)

    pid = os.fork()
    if pid == 0:
        with open(os.devnull, "wb") as sink:
            os.dup2(sink.fileno(), 1)
            os.dup2(sink.fileno(), 2)
            with patch.object(executor, "Application", CrashAfterRepair):
                os._exit(executor.main(["--repair-outage", shas["G"]]))
    _, status = os.waitpid(pid, 0)
    need(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 73,
         "real outage cutover crash boundary not reached")
    interrupted = coordinated.Journal().load()
    need(interrupted.mode == "outage" and interrupted.state == "active"
         and interrupted.phase == "app" and interrupted.candidate == shas["G"],
         "interrupted outage intent missing")
    executor.Application(executor.AppStore(), edge.SIGNERS, -1).verify(shas["G"])
    result = installed_executor("--repair-outage", shas["G"])
    need(result.returncode != 0, "unfinished outage journal did not block a release")
    result = installed_executor("--recover")
    need(result.returncode == 1 and b"TOR_V2_RECOVERY outage_stopped" in result.stdout,
         "journal recovery falsely reported healthy rollback")
    repair_witness(db, credentials, book, stopped=True,
                   previous=shas["F"], candidate=shas["G"])
    need(unit.Systemd().snapshot()["RestartUSec"] == "7s", "crash unit policy not restored")
    approve_outage(shas["F"], shas["G"])
    result = installed_executor("--repair-outage", shas["G"])
    need(result.returncode == 0 and
         ("TOR_V2_OUTAGE_REPAIRED " + shas["G"]).encode() in result.stdout,
         "freshly approved retry did not recover")
    repair_witness(db, credentials, book, stopped=False, previous=shas["G"])
    print("DEPLOY08D_REAL_INTERRUPTED_REPAIR_RECOVERED", flush=True)


def run():
    checkout = hosted_guard()
    os.umask(0o077)
    # Same compiled checkout, separate synthetic identities for fault fixtures.
    shas = {label: hashlib.sha256((checkout + ":vm:" + label).encode()).hexdigest()[:40]
            for label in "ABCD"}
    shas.update(REAL_CHAIN)
    key = LAB / "fixture-key"
    executor.command("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key))
    signers = LAB / "allowed-signers"
    signers.write_bytes(bundle.IDENTITY.encode() + b" " + key.with_suffix(".pub").read_bytes())
    signers.chmod(0o600)
    source = {label: fixture_release(label, sha, signers, key,
                                     broken=label == "C", late_failure=label == "E")
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
    # Outside both fixture ancestry patches and previous release locks.
    real_outage_acceptance(source, shas, db, credentials, book)
    print("DEPLOY08A_VM_ACCEPTANCE_OK", flush=True)


if __name__ == "__main__":
    run()
