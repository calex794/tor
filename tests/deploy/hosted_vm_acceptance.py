#!/usr/bin/env python3
"""Real hosted-VM fixture. No operational CLI or production main override.

Only this test process pins synthetic release identities to a fixture
allowlist, never a production helper, deployed wrapper, or shell main gate.
The lab application activation boundary runs the real signed migration
script and real systemd restart. The production rollback/verify and all
other components are used unchanged.
"""
from __future__ import annotations

from contextlib import contextmanager
import hashlib
import inspect
import os
from pathlib import Path
import pwd
import re
import shutil
import socket
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
# Real outage acceptance uses live API proofs through an authenticated lab-only
# Request adapter; the verifier and signed shell are not changed.
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


def vm_authenticated_request(url, *, headers):
    """Only the disposable VM gets an ephemeral Actions read-only API token.

    This function is embedded into the lab copy of release_ancestry.py, not
    the production module or signed shell. No comparison result is faked.
    """
    import os
    import re
    import stat
    from pathlib import Path
    from urllib.parse import urlsplit
    from urllib.request import Request as OriginalRequest

    target = urlsplit(url)
    if (target.scheme != "https" or target.netloc != "api.github.com"
            or not re.fullmatch(
                r"/repos/calex794/tor/(?:git/ref/heads/main|compare/[a-f0-9]{40}\.\.\.[a-f0-9]{40})",
                target.path)):
        raise RuntimeError("fixture API destination refused")
    directory = Path("/root/tor-deploy-lab")
    parent = directory.lstat()
    if (os.geteuid() != 0 or not stat.S_ISDIR(parent.st_mode)
            or parent.st_uid != 0 or parent.st_gid != 0
            or stat.S_IMODE(parent.st_mode) != 0o700
            or not (directory / "approved-checkout").is_file()):
        raise RuntimeError("fixture API outside guarded VM")
    fd = os.open(directory / "actions-read-token", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_gid != 0
                or stat.S_IMODE(info.st_mode) != 0o600 or info.st_nlink != 1):
            raise RuntimeError("unsafe fixture API token file")
        token = stream.read(8193)
    # Accept opaque RFC 6750 bearer syntax (including JWT-style dots),
    # not a particular historical GitHub token prefix or length.
    if len(token) > 8192 or not re.fullmatch(rb"[A-Za-z0-9._~+/-]+=*", token):
        raise RuntimeError("invalid fixture API bearer credential")
    request = OriginalRequest(url, headers=headers)
    # Do not send the authorization header on redirects.
    request.add_unredirected_header("Authorization", "Bearer " + token.decode("ascii"))
    return request


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
                            "F": b"RestartSec=7s", "G": b"RestartSec=8s",
                            "H": b"RestartSec=9s"}[label]))
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
    # Only the VM-local Request adapter authenticates the live API calls.
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
        elif (line.startswith(b"REFUSE_RELEASE_ANCESTRY:")
              or line == b"TOR_RELEASE_REFUSED: ancestry proof failed: HTTPError"
              or line == b"TOR_V2_REFUSED: HTTPError"):
            print("DEPLOY08D_ANCESTRY_REFUSED", flush=True)
    return result


def approve_outage(previous, candidate):
    first.new_file(executor.OUTAGE_APPROVAL,
                   (previous + "\n" + candidate + "\n").encode("ascii"), 0o600)


def repair_witness(db, credentials, book, *, stopped, previous, candidate=None,
                   previous_edge=None):
    app = executor.AppStore()
    need(app.current() == previous, "unexpected outage app pointer")
    need(edge.EdgeStore(edge.ROOT).active_sha() == (previous_edge or previous),
         "unexpected outage edge pointer")
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


def sudo_release(*args):
    return subprocess.run(
        ["runuser", "-u", "tor-deploy", "--", "sudo", "-n", str(transition.WRAPPER), *args],
        env=executor.SAFE_ENV, capture_output=True, timeout=240)


class LabSSH:
    """Loopback-only transport with a pinned, ephemeral fixture host key."""
    HOST = "127.0.0.1"
    PORT = 22222
    DEST = "tor-deploy@127.0.0.1"

    def __init__(self, key, known_hosts, wrong_hosts):
        self.key = key
        self.known_hosts = known_hosts
        self.wrong_hosts = wrong_hosts

    def options(self, *, wrong=False):
        hosts = self.wrong_hosts if wrong else self.known_hosts
        return ["-F", "/dev/null", "-i", str(self.key),
                "-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes",
                "-o", "StrictHostKeyChecking=yes", "-o", "GlobalKnownHostsFile=/dev/null",
                "-o", "UserKnownHostsFile=" + str(hosts), "-o", "ConnectTimeout=5"]

    def run(self, remote, *, wrong=False, timeout=240):
        return subprocess.run(["ssh", "-p", str(self.PORT), *self.options(wrong=wrong),
                               self.DEST, remote], env=executor.SAFE_ENV,
                              capture_output=True, timeout=timeout)

    def stage(self, source, sha):
        bundle.validate_sha(sha)
        inbox = "/opt/tor/incoming/v2/" + sha
        # Same mkdir/permission and scp -p contract as the production workflow.
        result = self.run("umask 077; test -d /opt/tor/incoming && "
                          "test -w /opt/tor/incoming && mkdir -p '" + inbox +
                          "' && test ! -L '" + inbox + "' && chmod 0700 '" + inbox + "'")
        need(result.returncode == 0, "SSH inbox preparation failed")
        result = subprocess.run(
            ["scp", "-p", "-P", str(self.PORT), *self.options(),
             *(str(source / name) for name in sorted(bundle.FILES)), self.DEST + ":" + inbox + "/"],
            env=executor.SAFE_ENV, capture_output=True, timeout=240)
        need(result.returncode == 0, "SCP signed bundle transfer failed")

    def activate(self, sha):
        bundle.validate_sha(sha)
        return self.run("grep -Fxq '# TOR RELEASE PROTOCOL 2' /usr/local/sbin/tor-one-click-release"
                        ' && test "$(cat /var/lib/tor-release/release-format)" = 2'
                        " && sudo -n /usr/local/sbin/tor-one-click-release '" + sha + "'")


    def verify_release(self, sha):
        # Independently exercise the final workflow checks as the SSH user,
        # after the root executor exits. No sudo, trusted-helper imports
        # or test-only readiness substitute in this remote check.
        bundle.validate_sha(sha)
        return self.run(
            'test "$(readlink -f /opt/tor/current)" = ' + "'/opt/tor/releases/" + sha + "' && "
            'systemctl is-active --quiet tor-web.service && '
            'test "$(curl --disable -fsS --noproxy \'*\' --max-time 5 '
            'http://127.0.0.1:3000/readyz)" = ok && '
            'test "$(cat /var/lib/tor-release/pg-activated)" = PG_ACTIVATED && '
            'test "$(readlink -f /var/lib/tor-edge/current)" = ' +
            "'/var/lib/tor-edge/releases/" + sha + "' && " +
            'test "$(curl --disable -fsS --noproxy \'*\' --max-time 5 --resolve '
            'pandaolate.ru:443:127.0.0.1 https://pandaolate.ru/readyz)" = ok')


@contextmanager
def lab_ssh(key):
    # Called only after the hosted_guard in run(). No production account,
    # key, sshd configuration, remote destination or host service is used.
    directory = LAB / "ssh"
    directory.mkdir(mode=0o700)
    deploy = pwd.getpwnam("tor-deploy")
    home = Path(deploy.pw_dir)
    need(home == Path("/home/tor-deploy") and home.resolve() == home
         and home.stat().st_uid == deploy.pw_uid, "unexpected fixture deploy home")
    # SSH needs a valid command shell; password login remains impossible.
    executor.command("usermod", "--shell", "/bin/bash", "--password", "*", "tor-deploy")
    sshdir = home / ".ssh"
    sshdir.mkdir(mode=0o700)
    client_pub = key.with_suffix(".pub").read_bytes().strip()
    authorized = sshdir / "authorized_keys"
    authorized.write_bytes(b"restrict " + client_pub + b"\n")
    authorized.chmod(0o600)
    for item in (sshdir, authorized):
        os.chown(item, deploy.pw_uid, deploy.pw_gid)
    hostkey = directory / "host"
    executor.command("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(hostkey))
    host_pub = hostkey.with_suffix(".pub").read_bytes().split()[:2]
    client_fields = client_pub.split()[:2]
    host_alias = b"[127.0.0.1]:22222 "
    known_hosts = directory / "known_hosts"
    wrong_hosts = directory / "wrong_hosts"
    first.new_file(known_hosts, host_alias + b" ".join(host_pub) + b"\n", 0o600)
    first.new_file(wrong_hosts, host_alias + b" ".join(client_fields) + b"\n", 0o600)
    executor.command("ssh-keygen", "-F", "[127.0.0.1]:22222", "-f", str(known_hosts))
    config = directory / "sshd_config"
    first.new_file(config, (
        "Port 22222\nListenAddress 127.0.0.1\nAddressFamily inet\n"
        "HostKey " + str(hostkey) + "\nPidFile " + str(directory / "sshd.pid") + "\n"
        "AuthorizedKeysFile .ssh/authorized_keys\n"
        "AllowUsers tor-deploy\nPermitRootLogin no\nAuthenticationMethods publickey\n"
        "PubkeyAuthentication yes\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n"
        "PermitEmptyPasswords no\nUsePAM no\nStrictModes yes\n"
        "DisableForwarding yes\nPermitTTY no\nPermitUserEnvironment no\n"
        "PrintMotd no\nPrintLastLog no\nUseDNS no\nLogLevel ERROR\n"
        "Subsystem sftp internal-sftp\n").encode(), 0o600)
    executor.command("/usr/sbin/sshd", "-t", "-f", str(config))
    with (directory / "sshd.log").open("wb") as log:
        daemon = subprocess.Popen(["/usr/sbin/sshd", "-D", "-e", "-f", str(config)],
                                  env=executor.SAFE_ENV, stdout=log, stderr=log)
        try:
            for _ in range(40):
                need(daemon.poll() is None, "loopback sshd exited")
                try:
                    with socket.create_connection(("127.0.0.1", 22222), timeout=0.5):
                        break
                except OSError:
                    time.sleep(0.25)
            else:
                raise RuntimeError("loopback sshd did not start")
            listeners = executor.command("ss", "-H", "-ltn", "sport = :22222").stdout.splitlines()
            need(len(listeners) == 1 and b"127.0.0.1:22222" in listeners[0],
                 "fixture SSH not exclusively loopback")
            transport = LabSSH(key, known_hosts, wrong_hosts)
            need(transport.run("id -un").stdout == b"tor-deploy\n", "fixture SSH identity differs")
            print("DEPLOY08F_LOOPBACK_SSH_READY", flush=True)
            yield transport
        finally:
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait(timeout=10)
            authorized.unlink(missing_ok=True)
            hostkey.unlink(missing_ok=True)


def visudo_diagnostic(stage, result):
    # The VM has only disposable OS sudoers and fixture rules. Bound and
    # escape the diagnostic, never print an environment or a secret file.
    print("DEPLOY08E_VISUDO_DIAGNOSTIC stage=" + stage
          + " exit=" + str(result.returncode)
          + " stdout=" + repr(result.stdout[:2048])
          + " stderr=" + repr(result.stderr[:2048]), flush=True)


def parsed_vm_sudoers_paths(output):
    # Only canonical standard sudoers files reported as parsed successfully
    # by the real parser are eligible for fixture-only mode normalization.
    for line in output.splitlines():
        match = re.fullmatch(
            rb"(/etc/sudoers(?:\.d/[A-Za-z0-9_-]+)?): parsed OK", line)
        if match:
            yield Path(match.group(1).decode("ascii"))


def require_vm_sudoers(stage, *, normalize_runner_modes=False):
    result = executor.command("visudo", "-c", check=False)
    if result.returncode == 0:
        return
    visudo_diagnostic(stage, result)
    if normalize_runner_modes:
        # GitHub-hosted images may create a root-owned sudoers include
        # with mode 0644. Only fix that precise mode discrepancy on this
        # disposable VM, after proving the full syntax independently.
        # No rule text, ownership, symlink or writable file is changed.
        syntax = executor.command("visudo", "-c", "-f", "/etc/sudoers", check=False)
        if syntax.returncode != 0:
            visudo_diagnostic("existing_syntax", syntax)
            raise RuntimeError("invalid pre-existing VM sudoers syntax")
        for path in sorted(set(parsed_vm_sudoers_paths(syntax.stdout + b"\n" + syntax.stderr))):
            if path.resolve(strict=True) != path:
                continue
            fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            try:
                info = os.fstat(fd)
                if (stat.S_ISREG(info.st_mode) and info.st_uid == 0 and info.st_gid == 0
                        and info.st_nlink == 1 and stat.S_IMODE(info.st_mode) == 0o644):
                    os.fchmod(fd, 0o440)
                    os.fsync(fd)
                    print("DEPLOY08E_VM_SUDOERS_MODE_NORMALIZED " + str(path),
                          flush=True)
            finally:
                os.close(fd)
        result = executor.command("visudo", "-c", check=False)
        if result.returncode == 0:
            return
        visudo_diagnostic("existing_after_mode_check", result)
    raise RuntimeError("VM sudoers validation failed: " + stage)


def make_legacy_fixture(sha, key):
    # The first-install/normal rollback fixture is already healthy. Rebuild
    # only its lab metadata into the real v1 layout, without replacing the
    # live executable, and preserve the removed v2 metadata privately.
    app = executor.AppStore()
    release = app.release(sha)
    preserve = LAB / "pre-adoption-v2-metadata"
    preserve.mkdir(mode=0o700)
    for item in release.iterdir():
        if item.name not in bundle.BINARIES:
            item.rename(preserve / item.name)
    manifest = ("TOR_RELEASE_SHA " + sha + "\n").encode()
    for name in bundle.BINARIES:
        with (release / name).open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        entry = (digest + "  " + name + "\n").encode()
        first.new_file(release / (name + ".sha256"), entry)
        manifest += entry
    path = release / "tor-release-manifest"
    first.new_file(path, manifest)
    executor.command("ssh-keygen", "-Y", "sign", "-f", str(key), "-n",
                     "tor-release", str(path))
    (release / "tor-release-manifest.sig").chmod(0o644)
    executor.command("bash", "-c",
                     'ssh-keygen -Y verify -f "$1" -I tor-release -n tor-release -s "$2" < "$3"',
                     "verify", str(edge.SIGNERS), str(release / "tor-release-manifest.sig"),
                     str(path))
    need(not os.path.lexists(release / bundle.MANIFEST), "legacy v2 manifest still present")

    # Convert the known fixture vhost to a real unmanaged proxy. Preserve
    # the old managed directory in the private disposable lab, only after
    # nginx has reloaded and no longer includes its contents.
    nginx = transition.adoption.ProductionAdoption()
    vhost = first.VHOST
    info, old = transition.adoption.checked_vhost(vhost)
    includes = {b"include /var/lib/tor-edge/current/" + name.encode() + b";"
                for name in edge.SNIPPETS}
    removed = [line.strip() for line in old.splitlines() if line.strip() in includes]
    need(len(removed) == 3 and set(removed) == includes, "unexpected fixture vhost")
    legacy = b"\n".join(line for line in old.splitlines() if line.strip() not in includes) + b"\n"
    transition.adoption.atomic_replace(vhost, old, legacy, info)
    nginx.test()
    nginx.reload()
    nginx.loaded(vhost)
    need(nginx.snapshot("/catalog").status == 200 and
         not nginx.snapshot("/_tor-fallback/game.js").headers.get(edge.MARKER.lower()),
         "legacy vhost not actually active")
    shutil.move(str(edge.ROOT), str(LAB / "pre-adoption-managed-edge"))
    wrapper = (ROOT / "ops/one-click-release-vps.sh").read_bytes()
    need(transition.V1_MARKER not in wrapper.splitlines(), "unexpected fixture wrapper marker")
    wrapper = wrapper.rstrip(b"\n") + b"\n" + transition.V1_MARKER + b"\n"
    first.new_file(transition.WRAPPER, wrapper, 0o755)
    sudoers = Path("/etc/sudoers.d/tor-one-click-release")
    rule = (b"# TOR one-click release: restricted root helper only\n"
            b"tor-deploy ALL=(root) NOPASSWD: /usr/local/sbin/tor-one-click-release *\n")
    staged_rule = LAB / "approved-legacy-sudoers"
    first.new_file(staged_rule, rule, 0o440)
    syntax = executor.command("visudo", "-c", "-f", str(staged_rule), check=False)
    if syntax.returncode != 0:
        visudo_diagnostic("new_rule_syntax", syntax)
        raise RuntimeError("invalid fixture TOR sudoers rule")
    first.new_file(sudoers, rule, 0o440)
    info = sudoers.lstat()
    need(stat.S_ISREG(info.st_mode) and info.st_uid == 0 and info.st_gid == 0
         and info.st_nlink == 1 and stat.S_IMODE(info.st_mode) == 0o440,
         "unexpected fixture TOR sudoers ownership or mode")
    require_vm_sudoers("after_tor_rule")
    print("DEPLOY08E_VM_TOR_SUDOERS_OK", flush=True)
    return legacy, wrapper, sudoers, rule


def legacy_bootstrap_acceptance(source, shas, files, key, signers, inbox,
                                db, credentials, book, transport):
    app = executor.Application(executor.AppStore(), edge.SIGNERS, -1)
    baseline = shas["B"]
    # Pin the actual live main head, not a synthetic allowlist. A main
    # advance during bootstrap is a legitimate refusal. Only authenticated
    # HTTP transport is fixture-specific; exact equality remains required.
    head_ref = executor.ancestry.public_get("/repos/calex794/tor/git/ref/heads/main")
    head = head_ref.get("object", {}).get("sha")
    need(executor.ancestry.valid_sha(head), "invalid live bootstrap head")
    need(head not in shas.values(), "bootstrap identity must be distinct")
    shas["H"] = head
    executor.require_forward(head, shas["G"])
    source["H"] = fixture_release("H", head, signers, key)
    target = inbox / head
    legacy, wrapper, sudoers, rule = make_legacy_fixture(baseline, key)
    # A wrong pinned host key must refuse before even creating the inbox.
    pid = executor.service_pid()
    before = coordinated.Journal().load()
    wrong = transport.run("mkdir -p '" + str(target) + "'", wrong=True)
    need(wrong.returncode == 255 and b"Host key verification failed" in wrong.stderr
         and not os.path.lexists(target), "incorrect host key did not block transfer")
    print("DEPLOY08F_WRONG_HOST_KEY_REFUSED", flush=True)
    transport.stage(source["H"], head)
    bundle.verify(target, head, edge.SIGNERS)
    need(stat.S_IMODE(target.stat().st_mode) == 0o700 and
         target.stat().st_uid == pwd.getpwnam("tor-deploy").pw_uid and
         app.current() == baseline and executor.service_pid() == pid and
         coordinated.Journal().load() == before and
         database_witness() == db and secrets_witness() == credentials,
         "SSH stage-only changed live state")
    app.verify(baseline)
    print("DEPLOY08F_SSH_STAGE_ONLY_OK", flush=True)
    vhost = first.VHOST
    pin = transition.adoption.hash_bytes(legacy)
    approved = LAB / "approved-v2-sources"
    approved.mkdir(mode=0o700)
    files = dict(files)
    files["wrapper.sh"] = (ROOT / "ops/one-click-release-v2-wrapper.sh").read_bytes()
    for name, data in files.items():
        first.new_file(approved / name, data, 0o600)

    def exact_live_main(sha):
        if executor.ancestry.require_on_main(executor.ancestry.public_get, sha) != sha:
            raise executor.ancestry.AncestryError("candidate is no longer GitHub main")

    def assert_old():
        need(edge.read_small(transition.WRAPPER, 1024 * 1024) == wrapper and
             edge.read_small(vhost, transition.adoption.MAX_CONFIG) == legacy and
             edge.read_small(sudoers, 8192) == rule and
             not os.path.lexists(transition.MARKER) and
             not os.path.lexists(transition.BACKUP) and
             not os.path.lexists(transition.HELPERS) and
             app.current() == baseline, "refused bootstrap modified legacy state")

    with patch.object(executor, "require_main", exact_live_main):
        executor.command("systemctl", "stop", executor.APP)
        try:
            transition.bootstrap(approved, head, vhost, pin)
        except subprocess.CalledProcessError as exc:
            need(tuple(exc.cmd) == ("systemctl", "is-active", "--quiet", executor.APP),
                 "bootstrap refused for a reason other than stopped legacy web")
            assert_old()
            need(not os.path.lexists(executor.STATE / "bootstrap-bundles"),
                 "unhealthy bootstrap staged candidate")
        else:
            raise AssertionError("unhealthy legacy site allowed bootstrap")
        print("DEPLOY08E_REAL_V1_OUTAGE_REFUSED", flush=True)
        executor.command("systemctl", "start", executor.APP)
        wait_app(app, baseline)
        # Wrong vhost hash must not replace the legacy sudo route, even
        # after the signed candidate has been copied and checked.
        try:
            transition.bootstrap(approved, head, vhost, "0" * 64)
        except transition.InstallError as exc:
            need("vhost differs from approved hash" in str(exc), "unexpected pin refusal")
            assert_old()
        else:
            raise AssertionError("unapproved legacy vhost accepted")
        print("DEPLOY08E_REAL_VHOST_PIN_REFUSED", flush=True)
        pid = executor.service_pid()
        before = transition.adoption.ProductionAdoption().snapshot("/catalog")
        transition.bootstrap(approved, head, vhost, pin)
        executor.require_v2_marker()
        app.verify(baseline)
        need(executor.service_pid() == pid, "bootstrap restarted Rust")
        need(edge.read_small(transition.BACKUP, 1024 * 1024) == wrapper and
             stat.S_IMODE(transition.BACKUP.stat().st_mode) == 0o600 and
             edge.read_small(transition.WRAPPER, 1024 * 1024) == files["wrapper.sh"] and
             edge.read_small(sudoers, 8192) == rule, "legacy backup or sudo route differs")
        store = edge.EdgeStore(edge.ROOT)
        need(store.active_sha() == head, "adopted edge identity differs")
        nginx = transition.adoption.ProductionAdoption()
        nginx.probe(head, store.releases / head)
        transition.adoption.compare_catalog(before, nginx.snapshot("/catalog"))
        need(edge.read_small(store.root / "adoption-backups" / (pin + ".conf"),
                             transition.adoption.MAX_CONFIG) == legacy,
             "original vhost backup differs")
        need(database_witness() == db and secrets_witness() == credentials and
             b"DEPLOY08A retained book" in http("/books/" + book),
             "bootstrap changed retained state")
        try:
            transition.bootstrap(approved, head, vhost, pin)
        except transition.InstallError as exc:
            need("already configured" in str(exc), "unexpected repeat bootstrap refusal")
        else:
            raise AssertionError("completed bootstrap could be repeated")

    refused = sudo_release("--recover")
    need(refused.returncode == 2 and b"TOR_V2_REFUSED: one full commit SHA required" in refused.stderr,
         "installed sudo wrapper exposed root recovery")
    print("DEPLOY08E_REAL_V1_V2_BOOTSTRAP_OK", flush=True)
    return head


def real_outage_acceptance(source, shas, db, credentials, book, key, signers, transport):
    need(executor.AppStore().current() == shas["B"], "unexpected real-chain baseline")
    for prev, candidate in zip("BEF", "EFG"):
        executor.require_forward(shas[candidate], shas[prev])
    print("DEPLOY08D_AUTHENTICATED_LIVE_ANCESTRY_OK", flush=True)
    files = {name: (ROOT / "tools" / name).read_bytes() for name in transition.MODULES}
    # Preserve the actual verifier, active pointer check and CLI, changing
    # only Request construction in the private disposable copy. The signed
    # go-live shell executes that copy under python -I as on the VPS.
    original = files["release_ancestry.py"]
    anchor = b"\ndef public_get(path, params=None):"
    need(original.count(anchor) == 1, "fixture ancestry integration changed")
    adapter = inspect.getsource(vm_authenticated_request).encode()
    files["release_ancestry.py"] = original.replace(
        anchor, b"\n" + adapter + b"\nRequest = vm_authenticated_request\n" + anchor)
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

    adopted_edge = legacy_bootstrap_acceptance(source, shas, files, key, signers, inbox,
                                               db, credentials, book, transport)

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
         edge.EdgeStore(edge.ROOT).active_sha() == adopted_edge, "refusal changed live state")
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
                   previous=shas["B"], candidate=shas["E"], previous_edge=adopted_edge)
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
    # verified G, before the coordinator can durably commit. The Git proof
    # is live and authenticated; shell, service and database are real.
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

    # H was staged for adoption only; it has never been the active app.
    # A modified writable inbox must not become a root-owned release.
    candidate = shas["H"]
    target = inbox / candidate
    active = shas["G"]
    pid = executor.service_pid()
    previous = coordinated.Journal().load()
    need(not os.path.lexists(executor.TOR / "releases" / candidate),
         "fixture candidate already published")
    tamper = transport.run("printf '\\n# tampered fixture\\n' >> '" +
                           str(target / "fallback-game.js") + "'")
    need(tamper.returncode == 0, "fixture tamper did not execute")
    refused = transport.activate(candidate)
    need(refused.returncode != 0 and b"TOR_V2_REFUSED: BundleError" in refused.stderr and
         not os.path.lexists(executor.TOR / "releases" / candidate) and
         executor.AppStore().current() == active and executor.service_pid() == pid and
         edge.EdgeStore(edge.ROOT).active_sha() == active and
         coordinated.Journal().load() == previous and
         database_witness() == db and secrets_witness() == credentials,
         "tampered SSH inbox was not refused without cutover")
    print("DEPLOY08F_TAMPERED_INBOX_REFUSED", flush=True)
    # Restore the approved bundle over SCP, then execute the exact wrapper
    # command over the actual SSH connection as the deployment account.
    transport.stage(source["H"], candidate)
    result = transport.activate(candidate)
    need(result.returncode == 0 and
         ("TOR_ONE_CLICK_RELEASE_V2_OK " + shas["H"]).encode() in result.stdout,
         "installed sudo wrapper did not complete signed normal release")
    record = coordinated.Journal().load()
    need(record.mode == "normal" and record.state == "committed" and
         record.candidate == shas["H"], "normal sudo release not committed")
    executor.Application(executor.AppStore(), edge.SIGNERS, -1).verify(shas["H"])
    need(edge.EdgeStore(edge.ROOT).active_sha() == shas["H"] and
         database_witness() == db and secrets_witness() == credentials and
         b"DEPLOY08A retained book" in http("/books/" + book),
         "normal sudo release changed retained state")
    pid = executor.service_pid()
    repeated = transport.activate(shas["H"])
    need(repeated.returncode == 0 and
         ("TOR_V2_ALREADY_ACTIVE " + shas["H"]).encode() in repeated.stdout and
         executor.service_pid() == pid, "same signed SSH release not idempotent")
    print("DEPLOY08E_REAL_SUDO_SIGNED_UPDATE_OK", flush=True)
    print("DEPLOY08F_REAL_SSH_SIGNED_UPDATE_OK", flush=True)
    need(transport.verify_release(candidate).returncode == 0,
         "independent SSH post-release witness failed")
    need(transport.verify_release(active).returncode != 0,
         "SSH post-release witness accepted stale SHA")
    print("DEPLOY08G_REMOTE_VERIFY_OK", flush=True)


def run():
    checkout = hosted_guard()
    # Read the opaque Actions credential via stdin, not sudo env propagation.
    # No subprocess receives it through environment or arguments.
    need("TOR_VM_GITHUB_TOKEN" not in os.environ, "fixture token leaked into environment")
    token = sys.stdin.buffer.read(8193)
    need(bool(token), "read-only Actions token input missing")
    need(len(token) <= 8192 and re.fullmatch(rb"[A-Za-z0-9._~+/-]+=*", token),
         "invalid Actions bearer credential syntax")
    os.umask(0o077)
    # Check the existing runner configuration before any TOR mutation.
    require_vm_sudoers("existing", normalize_runner_modes=True)
    print("DEPLOY08E_VM_EXISTING_SUDOERS_OK", flush=True)
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
    # Keep the ephemeral root-private lab key for later signing and loopback
    # SSH authentication, as the workflow uses the same key for both. The
    # final cleanup removes it; no production key is involved.
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
    # Outside the synthetic ancestry patches and prior release locks.
    # The read-only Actions token is never passed to the installed executor,
    # signed shell, or database through their environment or command line.
    token_path = LAB / "actions-read-token"
    first.new_file(token_path, token, 0o600)
    del token
    try:
        with patch.object(executor.ancestry, "Request", vm_authenticated_request), lab_ssh(key) as transport:
            real_outage_acceptance(source, shas, db, credentials, book, key, signers, transport)
    finally:
        key.unlink(missing_ok=True)
        key.with_suffix(".pub").unlink(missing_ok=True)
        token_path.unlink(missing_ok=True)
        edge.sync_dir(LAB)
    print("DEPLOY08A_VM_ACCEPTANCE_OK", flush=True)


if __name__ == "__main__":
    run()
