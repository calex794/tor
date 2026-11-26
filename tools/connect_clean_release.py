#!/usr/bin/env python3
"""DEPLOY-06c: fresh-host v2 helper installation, no release or SSH key rotation."""
from __future__ import annotations

import argparse
import base64
import binascii
import os
from pathlib import Path
import pwd
import re
import stat
import subprocess
import sys
import tempfile

import activate_clean_install as first
import admit_clean_install as admission
import install_release_v2 as installer
import managed_edge_release as edge
import release_executor_v2 as executor

SUDOERS = Path("/etc/sudoers.d/tor-one-click-release")
RULE = b"# TOR one-click release: restricted root helper only\ntor-deploy ALL=(root) NOPASSWD: /usr/local/sbin/tor-one-click-release *\n"


class Refused(ValueError):
    pass


def signer_key(data):
    match = re.fullmatch(rb"tor-release ssh-ed25519 ([A-Za-z0-9+/=]+)(?: [^\r\n]{0,200})?\n?", data)
    if match is None:
        raise Refused("exactly one pinned Ed25519 release signer required")
    encoded = match.group(1)
    try:
        raw = base64.b64decode(encoded, validate=True)
    except binascii.Error:
        raise Refused("invalid public key") from None
    prefix = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20"
    if not raw.startswith(prefix) or len(raw) != len(prefix) + 32:
        raise Refused("invalid Ed25519 public key wire format")
    return b"ssh-ed25519 " + encoded


def check_authorized(data, expected):
    if not re.fullmatch(rb"restrict " + re.escape(expected) + rb"(?: [^\r\n]{0,200})?\n?", data):
        raise Refused("deploy account must have exactly the independently pinned restricted key")


def ensure_file(path, data, mode):
    if os.path.lexists(path):
        info = edge.trusted(path)
        if (info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != mode
                or edge.read_small(path, 1024 * 1024) != data):
            raise Refused("existing installation file differs")
        return
    installer.atomic_write(path, data, mode)


def verify_sudo(*, installed):
    result = executor.command("sudo", "-n", "-l", "-U", "tor-deploy", check=False)
    if not installed and result.returncode == 1 and b"may run the following" not in result.stdout:
        return
    lines = result.stdout.decode("utf-8").splitlines()
    indexes = [i for i, line in enumerate(lines) if "may run the following commands" in line]
    expected = "(root) NOPASSWD: /usr/local/sbin/tor-one-click-release *"
    if (result.returncode != 0 or len(indexes) != 1 or
            [line.strip() for line in lines[indexes[0]+1:] if line.strip()] != [expected]):
        raise Refused("unexpected effective deploy sudo permissions")


def check_account():
    account = pwd.getpwnam("tor-deploy")
    if (account.pw_uid == 0 or account.pw_gid == 0 or account.pw_dir != "/home/tor-deploy" or
            executor.command("id", "-G", "tor-deploy").stdout.split() != [str(account.pw_gid).encode()]):
        raise Refused("dedicated unprivileged deploy account without extra groups required")
    # Read the user-controlled SSH file with the user's own privileges.
    # No privileged traversal/writes inside that account's home.
    data = executor.command("runuser", "-u", "tor-deploy", "--", "head", "-c", "4097",
                            "/home/tor-deploy/.ssh/authorized_keys").stdout
    check_authorized(data, signer_key(edge.read_small(edge.SIGNERS, 4096)))
    return account


def incoming(account):
    path = executor.TOR / "incoming"
    edge.trusted(path.parent, directory=True)
    if not os.path.lexists(path):
        path.mkdir(mode=0o700)
        os.chown(path, account.pw_uid, account.pw_gid)
        path.chmod(0o700)
        edge.sync_dir(path.parent)
    info = path.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != account.pw_uid
            or info.st_gid != account.pw_gid or stat.S_IMODE(info.st_mode) != 0o700):
        raise Refused("unsafe incoming directory")


def bootstrap(source, sha, machine, pin, ipv4, lock_fd):
    files = installer.source_files(source)
    target, store = admission.require_local_install(first.WORKSPACE, sha, machine, pin, edge.SIGNERS)
    identity = {"format": 1, "sha": sha, "machine_id": machine,
                "manifest_sha256": pin, "ipv4": admission.address(ipv4), "external_accepted": False}
    journal = admission.Journal(first.WORKSPACE / admission.JOURNAL, identity)
    if journal.load() != "admitted":
        raise Refused("complete approved interface admission first")
    with store.locked():
        ops = admission.Operations(sha, ipv4, store, lock_fd)
        if ops.current() != admission.render(edge.read_small(target / "install-nginx.conf", 262144), ipv4):
            raise Refused("vhost differs from admitted template")
        ops.verify_public()
        executor.require_main(sha)
        if os.path.lexists(installer.BACKUP):
            raise Refused("legacy host must use UPDATE bootstrap")
        for path in (installer.HELPERS.parent, installer.WRAPPER.parent,
                     SUDOERS.parent, installer.STATE):
            edge.trusted(path, directory=True)
        executor.command("visudo", "-c")
        account = check_account()
        if os.path.lexists(installer.MARKER):
            raise Refused("release helper already active; do not re-bootstrap")
        if os.path.lexists(SUDOERS):
            ensure_file(SUDOERS, RULE, 0o440)
            verify_sudo(installed=True)
        else:
            verify_sudo(installed=False)
        # Check exact wrapper before granting sudo. Unknown existing helper
        # or rules are never replaced. Without the marker the wrapper refuses.
        installer.install_helpers(installer.HELPERS, files)
        ensure_file(installer.WRAPPER, files["wrapper.sh"], 0o755)
        incoming(account)
        with tempfile.TemporaryDirectory(dir="/root", prefix=".tor-sudo-check-") as directory:
            candidate = Path(directory) / "sudoers"
            candidate.write_bytes(RULE)
            candidate.chmod(0o440)
            executor.command("visudo", "-cf", str(candidate))
        ensure_file(SUDOERS, RULE, 0o440)
        executor.command("visudo", "-c")
        verify_sudo(installed=True)
        ops.verify_public()
        executor.require_main(sha)
        installer.publish_marker(installer.MARKER)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("target-machine-id", "sha", "manifest-sha256", "bind-ipv4"):
        parser.add_argument("--" + arg, required=True)
    parser.add_argument("--helper-source", type=Path, required=True)
    parser.add_argument("--connect-github", action="store_true", required=True)
    args = parser.parse_args()
    try:
        if os.geteuid() != 0 or "GITHUB_ACTIONS" in os.environ:
            raise Refused("root on approved VPS required")
        if args.helper_source.resolve(strict=True) != args.helper_source:
            raise Refused("canonical reviewed helper source required")
        os.umask(0o077)
        os.environ.clear()
        os.environ.update(executor.SAFE_ENV)
        with executor.release_lock() as fd:
            bootstrap(args.helper_source, args.sha, args.target_machine_id,
                      args.manifest_sha256, args.bind_ipv4, fd)
        print("TOR_CLEAN_V2_CONNECTED " + args.sha + " external_ssh_accepted=false")
        return 0
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as exc:
        print("TOR_CLEAN_V2_REFUSED: " + (str(exc) if isinstance(exc, Refused) else type(exc).__name__),
              file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
