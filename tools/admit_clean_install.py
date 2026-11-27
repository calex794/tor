#!/usr/bin/env python3
"""DEPLOY-06c: explicit IPv4 admission of a verified clean local install."""
from __future__ import annotations

import argparse
import ipaddress
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys

import activate_clean_install as first
import adopt_edge_vhost as adoption
import install_release_v2 as installer
import managed_edge_release as edge
import prepare_clean_install as clean
import release_executor_v2 as executor

JOURNAL = "public-admission.json"
LISTEN = re.compile(rb"(?m)^([ ]*)listen 127\.0\.0\.1:(80|443)([^;\r\n]*);$")
HSTS = "max-age=31536000"


class Refused(ValueError):
    pass


def address(value):
    ip = ipaddress.IPv4Address(value)
    if (str(ip) != value or ip.is_loopback or ip.is_unspecified or
            ip.is_multicast or ip.is_link_local or ip.is_reserved):
        raise Refused("explicit assigned non-loopback IPv4 address required")
    return str(ip)


def render(local, ipv4):
    """Add exactly four listen directives; preserve all other signed bytes."""
    ip = address(ipv4).encode("ascii")
    if len(LISTEN.findall(local)) != 4 or len(re.findall(rb"(?m)^\s*listen\s", local)) != 4:
        raise Refused("unsupported signed local listen template")
    def add(match):
        indent, port, flags = match.groups()
        return match.group() + b"\n" + indent + b"listen " + ip + b":" + port + flags + b";"
    return LISTEN.sub(add, local)


def require_assigned(ipv4, output):
    info = json.loads(output)
    found = [item for interface in info for item in interface.get("addr_info", [])
             if item.get("family") == "inet" and item.get("scope") == "global"
             and item.get("local") == ipv4]
    if len(found) != 1:
        raise Refused("approved IPv4 must be assigned to exactly one local interface")


def check_listeners(output, public=None):
    expected = {(addr, port) for addr in ("127.0.0.1",) for port in ("80", "443", "3000", "5432")}
    if public is not None:
        expected.update((public, port) for port in ("80", "443"))
    actual = set()
    for line in output.splitlines():
        fields = line.split()
        if len(fields) < 5:
            raise Refused("invalid listener inventory")
        addr, _, port = fields[3].rpartition(":")
        if port in {"80", "443", "3000", "5432"}:
            actual.add((addr, port))
    if actual != expected:
        raise Refused("unexpected or missing web/database listener")


def require_local_install(workspace, sha, machine, pin, signers):
    clean.validate_host_id(machine, Path("/etc/machine-id").read_text().strip())
    if (workspace.resolve(strict=True) != workspace or
            stat.S_IMODE(edge.trusted(workspace, directory=True).st_mode) != 0o700):
        raise Refused("private installation workspace required")
    record = clean.plan(sha, machine, pin)
    for name, expected in (
        ("install-plan.json", record),
        (first.ACTIVATION, json.loads(first.record_bytes(record, "publish", "local_ready"))),
    ):
        path = workspace / name
        if (stat.S_IMODE(edge.trusted(path).st_mode) != 0o600
                or json.loads(edge.read_small(path, 16384)) != expected):
            raise Refused("local installation has not completed acceptance")
    if edge.read_small(executor.STATE / "pg-activated", 64) != b"PG_ACTIVATED\n":
        raise Refused("real database activation required")
    target = executor.AppStore().release(sha)
    clean.verified_source(target, sha, signers, pin)
    if executor.AppStore().current() != sha:
        raise Refused("local installation release no longer active")
    store = edge.EdgeStore(edge.ROOT)
    if store.active_sha() != sha:
        raise Refused("local edge release no longer active")
    return target, store


class Journal:
    def __init__(self, path, identity):
        self.path, self.identity = path, identity

    def load(self):
        if not os.path.lexists(self.path):
            return None
        if stat.S_IMODE(edge.trusted(self.path).st_mode) != 0o600:
            raise Refused("unsafe admission journal")
        data = json.loads(edge.read_small(self.path, 16384))
        if type(data) is not dict or set(data) != {*self.identity, "state"}:
            raise Refused("invalid admission journal")
        state = data.pop("state")
        if data != self.identity or state not in ("active", "admitted", "rolled_back", "rollback_pending"):
            raise Refused("admission journal differs from approved operation")
        return state

    def save(self, state):
        installer.atomic_write(
            self.path, (json.dumps({**self.identity, "state": state},
                                   sort_keys=True) + "\n").encode(), 0o600)


def reconcile_local(journal, ops, local, candidate):
    journal.save("rollback_pending")
    current = ops.current()
    if current == candidate:
        ops.replace(candidate, local)
    elif current != local:
        raise Refused("ADMISSION_ROLLBACK_UNVERIFIED: vhost changed outside transaction")
    ops.reload()
    ops.verify_local()
    journal.save("rolled_back")


def admit(journal, ops, local, candidate, *, restore=False):
    previous = journal.load()
    if restore:
        if previous is None:
            raise Refused("no recorded admission to restore")
        reconcile_local(journal, ops, local, candidate)
        return "rolled_back"
    if previous == "admitted":
        if ops.current() != candidate:
            raise Refused("admitted vhost changed outside transaction")
        ops.verify_public()
        return previous
    if previous not in (None, "rolled_back"):
        raise Refused("interrupted admission; explicit restore-local required")
    if ops.current() != local:
        raise Refused("initial vhost differs from signed local template")
    ops.verify_local()
    ops.preflight_public()
    journal.save("active")
    try:
        ops.replace(local, candidate)
        ops.reload()
        ops.verify_public()
        journal.save("admitted")
    except (Exception, KeyboardInterrupt) as failure:
        try:
            reconcile_local(journal, ops, local, candidate)
        except (Exception, KeyboardInterrupt):
            raise Refused("ADMISSION_ROLLBACK_UNVERIFIED") from None
        raise Refused("ADMISSION_ROLLBACK_VERIFIED: candidate refused") from failure
    return "admitted"


class Operations:
    def __init__(self, sha, ipv4, store, lock_fd):
        self.sha, self.ipv4, self.store = sha, ipv4, store
        self.nginx = edge.ProductionNginx()
        self.app = executor.Application(executor.AppStore(), edge.SIGNERS, lock_fd)

    def current(self):
        if (not first.ENABLED.is_symlink() or first.ENABLED.lstat().st_uid != 0
                or os.readlink(first.ENABLED) != str(first.VHOST)):
            raise Refused("unexpected enabled vhost link")
        return adoption.checked_vhost(first.VHOST)[1]

    def replace(self, expected, data):
        info, actual = adoption.checked_vhost(first.VHOST)
        if actual != expected:
            raise Refused("vhost changed")
        adoption.atomic_replace(first.VHOST, expected, data, info)

    def reload(self):
        self.nginx.test()
        self.nginx.reload()

    def listeners(self, public=None):
        check_listeners(executor.command("ss", "-H", "-ltn").stdout.decode(), public)

    def loaded(self):
        dump = executor.command("nginx", "-T").stdout.decode()
        if dump.count("# configuration file " + str(first.ENABLED) + ":") != 1:
            raise Refused("approved vhost not loaded exactly once")
        active = "\n".join(line for line in dump.splitlines() if not line.lstrip().startswith("#"))
        if len(re.findall(r"\bserver\s*\{", active)) != 4:
            raise Refused("unexpected additional Nginx servers")

    def verify_local(self):
        self.nginx.test()
        self.loaded()
        self.listeners()
        # Even if Rust has failed independently, verify restoration of the
        # autonomous local HTTPS edge; do not falsely claim app recovery.
        self.nginx.probe(self.sha, self.store.releases / self.sha)

    def preflight_public(self):
        require_assigned(self.ipv4,
                         executor.command("ip", "-j", "-4", "address", "show", "scope", "global").stdout)
        self.app.verify(self.sha)
        executor.require_main(self.sha)

    def snapshot(self, route, port=443):
        scheme = "https" if port == 443 else "http"
        result = executor.command(
            "curl", "--disable", "--silent", "--show-error", "--noproxy", "*",
            "--http1.1", "--max-time", "8", "--max-filesize", "2097152",
            "--interface", self.ipv4, "--resolve", f"{executor.HOST}:{port}:{self.ipv4}",
            "--output", "/dev/null", "--dump-header", "-",
            f"{scheme}://{executor.HOST}{route}",
        )
        block = result.stdout.rstrip(b"\r\n").split(b"\r\n\r\n")[-1]
        first_line, *lines = block.split(b"\r\n")
        status = re.match(rb"HTTP/1\.[01] ([1-5][0-9]{2})(?: |$)", first_line)
        if not status:
            raise Refused("invalid admission HTTP response")
        headers = {}
        for line in lines:
            key, sep, value = line.partition(b":")
            if not sep:
                raise Refused("invalid admission header")
            headers.setdefault(key.decode("ascii").lower(), []).append(value.decode("ascii").strip())
        return int(status.group(1)), headers

    def verify_public(self):
        self.nginx.test()
        self.loaded()
        self.listeners(self.ipv4)
        self.app.verify(self.sha)
        for route, expected in (("/catalog", 200), ("/readyz", 403),
                                ("/healthz", 403), ("/metrics", 404),
                                ("/internal/metrics", 404),
                                ("/_tor-fallback/index.html", 404),
                                ("/_tor-fallback/game.js", 200)):
            status, headers = self.snapshot(route)
            if status != expected or headers.get("strict-transport-security") != [HSTS]:
                raise Refused("public-interface route or HSTS acceptance failed")
            if route == "/catalog" and not any(
                    "noindex" in value.lower() for value in headers.get("x-robots-tag", [])):
                raise Refused("noindex missing")
            if route.endswith("game.js") and headers.get(edge.MARKER.lower()) != [self.sha]:
                raise Refused("unexpected public edge generation")
        status, headers = self.snapshot("/catalog", 80)
        if status != 308 or headers.get("location") != ["https://pandaolate.ru/catalog"]:
            raise Refused("HTTP redirect failed")
        self.nginx.probe(self.sha, self.store.releases / self.sha)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("target-machine-id", "sha", "manifest-sha256", "bind-ipv4"):
        parser.add_argument("--" + arg, required=True)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--admit-ipv4", action="store_true")
    action.add_argument("--restore-local", action="store_true")
    args = parser.parse_args()
    try:
        if os.geteuid() != 0 or "GITHUB_ACTIONS" in os.environ:
            raise Refused("root on approved VPS required")
        ip = address(args.bind_ipv4)
        os.umask(0o077)
        os.environ.clear()
        os.environ.update(executor.SAFE_ENV)
        with executor.release_lock() as fd:
            target, store = require_local_install(first.WORKSPACE, args.sha,
                args.target_machine_id, args.manifest_sha256, edge.SIGNERS)
            local = edge.read_small(target / "install-nginx.conf", 262144)
            candidate = render(local, ip)
            identity = {"format": 1, "sha": args.sha, "machine_id": args.target_machine_id,
                        "manifest_sha256": args.manifest_sha256, "ipv4": ip,
                        "external_accepted": False}
            journal = Journal(first.WORKSPACE / JOURNAL, identity)
            with store.locked():
                state = admit(journal, Operations(args.sha, ip, store, fd),
                              local, candidate, restore=args.restore_local)
        print("TOR_IPV4_ADMISSION " + state + " external_accepted=false")
        return 0
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as exc:
        print("TOR_IPV4_REFUSED: " + (str(exc) if isinstance(exc, Refused) else type(exc).__name__),
              file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
